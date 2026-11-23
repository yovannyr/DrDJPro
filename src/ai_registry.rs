//! Versioned, per-installation AI preferences and transactionally linked vault entries.
//! All methods that touch storage or the network are called on the AI worker.
mod requests;
#[cfg(test)]
mod tests;
mod types;
use crate::ai_transport::{is_loopback, validate_base_url};
use crate::credentials::Vault;
use rusqlite::{OptionalExtension, Transaction};
use std::{collections::BTreeMap, path::Path, sync::Mutex};
pub use types::*;

pub struct AiRegistry {
    vault: Vault,
    usage: Mutex<BTreeMap<String, (u64, u64)>>,
}

fn sql_error(_: rusqlite::Error) -> String {
    "AI settings storage unavailable or busy".into()
}
fn random_id() -> Result<String, String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Secure random source unavailable")?;
    Ok(format!("{:032x}", u128::from_be_bytes(bytes)))
}
fn service(id: &str) -> String {
    format!("ai:{id}")
}
fn normalize_key(input: &str) -> Result<&str, String> {
    let trimmed = input.trim();
    let key = if trimmed
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("Bearer "))
    {
        trimmed[7..].trim_start_matches(' ')
    } else {
        trimmed
    };
    if key.is_empty()
        || key.eq_ignore_ascii_case("Bearer")
        || key.len() > 4096
        || !key.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err("Invalid API key. Paste one printable token.".into());
    }
    Ok(key)
}
fn label(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn validate_config(p: &AiProviderConfig, confirm_http: bool) -> Result<(), String> {
    if !label(&p.name, 128)
        || (!p.model.is_empty() && !label(&p.model, 256))
        || p.base_url.len() > 2048
        || p.capabilities.context_window == Some(0)
    {
        return Err("Enter a provider name and valid settings".into());
    }
    let url = validate_base_url(&p.base_url).map_err(|e| e.to_string())?;
    if !is_loopback(&url) && p.kind == AiKind::LocalEndpoint && !confirm_http {
        return Err("Confirm that this endpoint sends data outside this computer".into());
    }
    if p.kind == AiKind::Subscription
        && (!p.preset.access().subscription
            || url.as_str().trim_end_matches('/') != AiPreset::OpenAi.access().base_url
            || p.capabilities.embeddings)
    {
        return Err(
            "ChatGPT plan usage requires the official OpenAI endpoint and supports Responses only"
                .into(),
        );
    }
    if !is_loopback(&url)
        && url.scheme() == "http"
        && (!confirm_http || p.kind != AiKind::LocalEndpoint)
    {
        return Err("Remote HTTP requires explicit confirmation and a keyless endpoint".into());
    }
    Ok(())
}
fn load(tx: &Transaction<'_>) -> Result<AiSettings, String> {
    let json: Option<String> = tx
        .query_row(
            "SELECT CASE WHEN length(CAST(payload AS BLOB)) <= 262144 THEN payload ELSE '' END
         FROM ai_preferences WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    let Some(json) = json else {
        return Ok(AiSettings::default());
    };
    let value: AiSettings =
        serde_json::from_str(&json).map_err(|_| "Invalid or unsupported AI preferences")?;
    if value.version != 1 {
        return Err("Unsupported AI preferences version".into());
    }
    Ok(value)
}
fn persist(tx: &Transaction<'_>, state: &mut AiSettings) -> Result<(), String> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or("AI preferences revision exhausted")?;
    let data = serde_json::to_string(state).map_err(|_| "Invalid AI preferences")?;
    if data.len() > 262144 {
        return Err("AI settings exceed size limit".into());
    }
    tx.execute("INSERT INTO ai_preferences(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",[data]).map_err(sql_error)?;
    Ok(())
}
fn forget_bindings(state: &mut AiSettings, id: &str) {
    state.bindings.retain(|_, b| b.provider_id != id);
    state.consents.retain(|_, p| p != id);
}

impl AiRegistry {
    pub fn open(directory: &Path) -> Result<Self, String> {
        let vault = Vault::open(directory.join("ai-providers.sqlite3"))?;
        Self::from_vault(vault)
    }
    fn from_vault(vault: Vault) -> Result<Self, String> {
        vault.with_transaction(|tx|{
            tx.execute_batch("CREATE TABLE IF NOT EXISTS ai_preferences (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS ai_host (id INTEGER PRIMARY KEY CHECK(id=1), host TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS ai_accounts (id TEXT PRIMARY KEY, client TEXT NOT NULL, subject TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS ai_plan_permissions (id TEXT PRIMARY KEY REFERENCES ai_accounts(id) ON DELETE CASCADE, enabled INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS ai_welcome (id INTEGER PRIMARY KEY CHECK(id=1));").map_err(sql_error)?;
            load(tx)?; Ok(())
        })?;
        Ok(Self {
            vault,
            usage: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn snapshot(&self) -> Result<AiSettings, String> {
        self.vault.with_transaction(load)
    }
    /// Read settings and credential indicators at one consistent revision for the UI.
    pub fn overview(&self) -> Result<(AiSettings, Vec<bool>), String> {
        self.vault.with_transaction(|tx| {
            let settings = load(tx)?;
            let credentials = settings
                .providers
                .iter()
                .map(|p| self.vault.contains_in(tx, &service(&p.id)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((settings, credentials))
        })
    }
    pub fn persistent(&self) -> bool {
        self.vault.is_persistent()
    }
    pub fn take_legacy_count(&self) -> usize {
        self.vault.take_discarded_legacy_count()
    }
    pub fn has_credential(&self, id: &str) -> Result<bool, String> {
        self.vault.with_transaction(|tx| {
            load(tx)?.provider(id)?;
            self.vault.contains_in(tx, &service(id))
        })
    }
    fn change<T>(
        &self,
        revision: u64,
        edit: impl FnOnce(&Transaction<'_>, &mut AiSettings) -> Result<T, String>,
    ) -> Result<T, String> {
        self.vault.with_transaction(|tx| {
            let mut s = load(tx)?;
            if s.revision != revision {
                return Err("AI settings changed in another window. Reload before saving.".into());
            }
            let value = edit(tx, &mut s)?;
            persist(tx, &mut s)?;
            Ok(value)
        })
    }
    pub fn save(
        &self,
        revision: u64,
        mut draft: AiProviderConfig,
        key: Option<&str>,
        confirm_http: bool,
    ) -> Result<String, String> {
        draft.name = draft.name.trim().into();
        draft.model = draft.model.trim().into();
        if draft.kind == AiKind::Subscription {
            draft.capabilities.embeddings = false
        }
        validate_config(&draft, confirm_http)?;
        let key = key.map(normalize_key).transpose()?;
        let endpoint = validate_base_url(&draft.base_url).map_err(|e| e.to_string())?;
        draft.base_url = endpoint.as_str().trim_end_matches('/').into();
        if key.is_some()
            && (draft.kind == AiKind::Subscription
                || endpoint.scheme() == "http" && !is_loopback(&endpoint))
        {
            return Err("API keys cannot be used with this connection".into());
        }
        self.change(revision, |tx, s| {
            if draft.id.is_empty() {
                if s.providers.len() >= 64 {
                    return Err("Maximum of 64 AI providers".into());
                }
                if draft.kind == AiKind::ApiKey && key.is_none() {
                    return Err("Enter an API key".into());
                }
                draft.id = random_id()?;
            } else {
                let old = s.provider(&draft.id)?.clone();
                let new_destination = old.base_url != draft.base_url
                    || old.kind != draft.kind
                    || old.preset != draft.preset;
                if new_destination {
                    if old.kind == AiKind::Subscription
                        && tx
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM ai_plan_permissions WHERE id=?1)",
                                [&draft.id],
                                |row| row.get::<_, bool>(0),
                            )
                            .map_err(sql_error)?
                    {
                        return Err("Disconnect the account before changing its destination".into());
                    }
                    if draft.kind == AiKind::ApiKey && key.is_none() {
                        return Err("Enter a new key for the changed endpoint".into());
                    }
                    self.vault.delete_in(tx, &service(&draft.id))?;
                    tx.execute("DELETE FROM ai_accounts WHERE id=?1", [&draft.id])
                        .map_err(sql_error)?;
                }
                if new_destination || old.capabilities != draft.capabilities {
                    forget_bindings(s, &draft.id)
                }
                s.providers.retain(|p| p.id != draft.id);
            }
            if let Some(key) = key {
                self.vault
                    .store_in(tx, &service(&draft.id), key.as_bytes())?
            }
            draft.last_test = None;
            let id = draft.id.clone();
            s.providers.push(draft);
            Ok(id)
        })
    }
    pub fn duplicate(&self, revision: u64, id: &str) -> Result<String, String> {
        self.change(revision, |_, s| {
            if s.providers.len() >= 64 {
                return Err("Maximum of 64 AI providers".into());
            }
            let mut p = s.provider(id)?.clone();
            p.name = format!("{} (copy)", p.name.chars().take(28).collect::<String>());
            p.id = random_id()?;
            p.last_test = None;
            let id = p.id.clone();
            s.providers.push(p);
            Ok(id)
        })
    }
    pub fn set_local_only(&self, revision: u64, value: bool) -> Result<(), String> {
        self.change(revision, |_, s| {
            s.local_only = value;
            Ok(())
        })
    }
    pub fn bind(
        &self,
        revision: u64,
        feature: AiFeature,
        id: &str,
        model: &str,
        consent: bool,
    ) -> Result<(), String> {
        self.change(revision, |_, s| {
            let provider = s.provider(id)?;
            if !feature.supports(&provider.capabilities) || !label(model, 256) {
                return Err("Incompatible capability or missing model".into());
            }
            if !is_loopback(&validate_base_url(&provider.base_url).map_err(|e| e.to_string())?) {
                if s.local_only {
                    return Err("Local AIs only is enabled".into());
                }
                if !consent && !s.consented(feature, id) {
                    return Err("Consent to send track metadata is required".into());
                }
                s.consents.insert(feature, id.into());
            } else {
                s.consents.remove(&feature);
            }
            s.bindings.insert(
                feature,
                AiBinding {
                    provider_id: id.into(),
                    model: model.trim().into(),
                },
            );
            Ok(())
        })
    }
    pub fn unbind(&self, revision: u64, feature: AiFeature) -> Result<(), String> {
        self.change(revision, |_, s| {
            s.bindings.remove(&feature);
            s.consents.remove(&feature);
            Ok(())
        })
    }
    pub fn revoke(&self, revision: u64, feature: AiFeature, id: &str) -> Result<(), String> {
        self.change(revision, |_, s| {
            if s.consented(feature, id) {
                s.consents.remove(&feature);
            }
            Ok(())
        })
    }
    pub fn usage(&self, id: &str) -> (u64, u64) {
        self.usage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .copied()
            .unwrap_or_default()
    }
}
