use super::*;
use crate::ai_oauth::{self, Account, Tokens};
use crate::ai_transport::{self, AiDiscovery, AiError, AiReply, AiRequest};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

fn account(tx: &Transaction<'_>, id: &str) -> Result<Option<Account>, String> {
    tx.query_row(
        "SELECT client,subject FROM ai_accounts WHERE id=?1",
        [id],
        |r| {
            Ok(Account {
                client: r.get(0)?,
                subject: r.get(1)?,
            })
        },
    )
    .optional()
    .map_err(sql_error)
}
fn tokens(bytes: &[u8]) -> Result<Tokens, String> {
    serde_json::from_slice(bytes).map_err(|_| "Stored session is invalid. Sign in again.".into())
}
fn record_scope(tx: &Transaction<'_>, id: &str, enabled: bool) -> Result<(), String> {
    tx.execute("INSERT INTO ai_plan_permissions(id,enabled) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
        rusqlite::params![id,enabled]).map_err(sql_error)?;
    Ok(())
}
fn permit_network(settings: &AiSettings, p: &AiProviderConfig) -> Result<(), String> {
    validate_config(p, true)?;
    let base = validate_base_url(&p.base_url).map_err(|e| e.to_string())?;
    if settings.local_only && !is_loopback(&base) {
        return Err("Local AIs only is enabled".into());
    }
    Ok(())
}

impl AiRegistry {
    /// Only the three predefined loopback listeners are eligible for discovery.
    pub fn discover(&self, cancel: &AtomicBool) -> Result<Vec<AiDiscovery>, String> {
        let result = ai_transport::discover_local(cancel);
        if cancel.load(Ordering::Relaxed) {
            Err(AiError::Cancelled.to_string())
        } else {
            Ok(result)
        }
    }

    pub fn models(&self, id: &str, cancel: &AtomicBool) -> Result<Vec<String>, String> {
        let snapshot = self.snapshot()?;
        self.dispatch(
            &snapshot,
            id,
            &AiRequest::Models,
            cancel,
            &mut |_| {},
            Instant::now() + Duration::from_secs(8),
        )
        .map(|r| r.models)
    }

    pub fn test(&self, id: &str, cancel: &AtomicBool) -> Result<String, String> {
        let start = Instant::now();
        let s = self.snapshot()?;
        let p = s.provider(id)?;
        if p.model.is_empty() {
            return Err("Select a model before testing the connection".into());
        }
        let probe = AiRequest::Test {
            model: p.model.clone(),
            embedding: !p.capabilities.chat && p.capabilities.embeddings,
        };
        let result = self
            .dispatch(
                &s,
                id,
                &probe,
                cancel,
                &mut |_| {},
                start + Duration::from_secs(8),
            )
            .map(|_| format!("Connected ({} ms)", start.elapsed().as_millis()));
        let status = match &result {
            Ok(s) | Err(s) => s.clone(),
        };
        self.change(s.revision, |_, current| {
            current
                .providers
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("Provider removed")?
                .last_test = Some(status);
            Ok(())
        })?;
        result
    }

    pub(crate) fn dispatch(
        &self,
        settings: &AiSettings,
        id: &str,
        request: &AiRequest,
        cancel: &AtomicBool,
        on_delta: &mut dyn FnMut(&str),
        deadline: Instant,
    ) -> Result<AiReply, String> {
        let p = settings.provider(id)?;
        let mut previous_key: Option<Zeroizing<String>> = None;
        let mut refreshed = false;
        for attempt in 0..2 {
            if cancel.load(Ordering::Relaxed) {
                return Err(AiError::Cancelled.to_string());
            }
            if Instant::now() >= deadline {
                return Err(AiError::Timeout.to_string());
            }
            let secret = self.vault.with_transaction(|tx| {
                let current = load(tx)?;
                if current.revision != settings.revision {
                    return Err("AI settings changed. Try again.".into());
                }
                let provider = current.provider(id)?;
                permit_network(&current, provider)?;
                let raw = self
                    .vault
                    .retrieve_in(tx, &service(id))?
                    .map(Zeroizing::new);
                if p.kind == AiKind::Subscription {
                    let bytes = raw.as_deref().ok_or("Sign in to ChatGPT first")?;
                    return self.subscription_key(
                        tx,
                        id,
                        bytes,
                        previous_key.as_ref().map(|k| k.as_str()),
                        |registration, token| {
                            let renewed = ai_oauth::refresh(registration, token, cancel, deadline)?;
                            refreshed = true;
                            Ok(renewed)
                        },
                    );
                }
                match raw {
                    Some(bytes) => Ok(Some(Zeroizing::new(
                        std::str::from_utf8(&bytes)
                            .map_err(|_| "Invalid stored credential")?
                            .to_string(),
                    ))),
                    None if p.kind == AiKind::ApiKey => Err("API key required".into()),
                    None => Ok(None),
                }
            })?;
            if p.kind == AiKind::Subscription && secret.is_none() {
                return Err("ChatGPT plan usage has not been authorized".into());
            }
            let mut before_send = || {
                if self
                    .snapshot()
                    .map_err(|_| AiError::SettingsChanged)?
                    .revision
                    != settings.revision
                {
                    return Err(AiError::SettingsChanged);
                }
                let mut usage = self.usage.lock().unwrap_or_else(|p| p.into_inner());
                let counters = usage.entry(id.to_string()).or_default();
                counters.0 = counters.0.saturating_add(1);
                Ok(())
            };
            let mut streamed = false;
            let provider = ai_transport::AiProvider::new(
                p.protocol(),
                &p.base_url,
                secret.as_ref().map(|s| s.as_str()),
            )
            .map_err(|e| e.to_string())?;
            match provider.dispatch(request, cancel, deadline, &mut before_send, &mut |text| {
                streamed = true;
                on_delta(text)
            }) {
                Ok(reply) => {
                    let mut usage = self.usage.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(counters) = usage.get_mut(id) {
                        counters.1 = counters.1.saturating_add(reply.tokens);
                    }
                    return Ok(reply);
                }
                Err(AiError::Authentication)
                    if p.kind == AiKind::Subscription
                        && attempt == 0
                        && !streamed
                        && !refreshed =>
                {
                    previous_key = secret
                }
                Err(err) => return Err(err.to_string()),
            }
        }
        Err(AiError::Authentication.to_string())
    }

    // The caller holds the write transaction from reading the current token to
    // committing its replacement, including a successful scope downgrade.
    fn subscription_key(
        &self,
        tx: &Transaction<'_>,
        id: &str,
        bytes: &[u8],
        failed_token: Option<&str>,
        renew: impl FnOnce(&Account, &Tokens) -> Result<Tokens, String>,
    ) -> Result<Option<Zeroizing<String>>, String> {
        let mut token = tokens(bytes)?;
        if !token.plan_enabled() {
            return Ok(None);
        }
        if token.needs_refresh() || failed_token == Some(token.access_token.as_str()) {
            let registration = account(tx, id)?.ok_or("ChatGPT registration missing")?;
            token = renew(&registration, &token)?;
            let encoded = Zeroizing::new(
                serde_json::to_vec(&token).map_err(|_| "Session serialization failed")?,
            );
            self.vault.store_in(tx, &service(id), &encoded)?;
            record_scope(tx, id, token.plan_enabled())?;
        }
        Ok(token
            .plan_enabled()
            .then(|| Zeroizing::new(std::mem::take(&mut token.access_token))))
    }

    pub fn sign_in(&self, id: &str, cancel: &AtomicBool) -> Result<bool, String> {
        let (revision, host, previous) = self.vault.with_transaction(|tx| {
            let s = load(tx)?;
            let p = s.provider(id)?;
            permit_network(&s, p)?;
            if p.kind != AiKind::Subscription {
                return Err("Select the ChatGPT plan connection first".into());
            }
            let mut bytes = [0; 16];
            getrandom::fill(&mut bytes).map_err(|_| "Secure random source unavailable")?;
            bytes[6] = bytes[6] & 0x0f | 0x40;
            bytes[8] = bytes[8] & 0x3f | 0x80;
            let value = format!("{:032x}", u128::from_be_bytes(bytes));
            let host = format!(
                "urn:uuid:{}-{}-{}-{}-{}",
                &value[..8],
                &value[8..12],
                &value[12..16],
                &value[16..20],
                &value[20..]
            );
            tx.execute(
                "INSERT OR IGNORE INTO ai_host(id,host) VALUES(1,?1)",
                [host],
            )
            .map_err(sql_error)?;
            let stable: String = tx
                .query_row("SELECT host FROM ai_host WHERE id=1", [], |r| r.get(0))
                .map_err(sql_error)?;
            Ok((s.revision, stable, account(tx, id)?))
        })?;
        let (registration, token) = ai_oauth::sign_in(&host, previous.as_ref(), cancel, || {
            let current = self.snapshot()?;
            if current.revision != revision {
                return Err("AI settings changed. Start sign-in again.".into());
            }
            permit_network(&current, current.provider(id)?)
        })?;
        self.change(revision,|tx,s|{
            permit_network(s,s.provider(id)?)?;
            let encoded=Zeroizing::new(serde_json::to_vec(&token).map_err(|_|"Session serialization failed")?);
            self.vault.store_in(tx,&service(id),&encoded)?;
            tx.execute("INSERT INTO ai_accounts VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET client=excluded.client,subject=excluded.subject",
                rusqlite::params![id,registration.client,registration.subject]).map_err(sql_error)?;
            record_scope(tx,id,token.plan_enabled())?;
            Ok(token.plan_enabled())
        })
    }

    /// Scope status for display; secret material stays encrypted.
    pub fn plan_enabled(&self, id: &str) -> Result<bool, String> {
        self.vault.with_transaction(|tx| {
            let granted = tx
                .query_row(
                    "SELECT enabled FROM ai_plan_permissions WHERE id=?1",
                    [id],
                    |r| r.get::<_, bool>(0),
                )
                .optional()
                .map_err(sql_error)?
                .unwrap_or(false);
            Ok(granted && self.vault.contains_in(tx, &service(id))?)
        })
    }

    /// Show the ChatGPT plan welcome once for this installation.
    pub fn take_plan_welcome(&self) -> Result<bool, String> {
        self.vault.with_transaction(|tx| {
            Ok(tx
                .execute("INSERT OR IGNORE INTO ai_welcome(id) VALUES(1)", [])
                .map_err(sql_error)?
                == 1)
        })
    }

    // The local deletion is committed even if the remote service is unavailable.
    fn discard_session(
        &self,
        tx: &Transaction<'_>,
        s: &AiSettings,
        id: &str,
    ) -> Result<bool, String> {
        let mut confirmed = true;
        if s.provider(id)?.kind == AiKind::Subscription {
            confirmed = match self.vault.retrieve_in(tx, &service(id)) {
                Ok(None) => true,
                Err(_) => false,
                Ok(Some(bytes)) => {
                    let bytes = Zeroizing::new(bytes);
                    !s.local_only
                        && account(tx, id)?
                            .and_then(|a| {
                                tokens(&bytes)
                                    .ok()
                                    .map(|t| ai_oauth::revoke(&a, &t).is_ok())
                            })
                            .unwrap_or(false)
                }
            };
        }
        self.vault.delete_in(tx, &service(id))?;
        tx.execute("DELETE FROM ai_plan_permissions WHERE id=?1", [id])
            .map_err(sql_error)?;
        Ok(confirmed)
    }

    pub fn disconnect(&self, revision: u64, id: &str) -> Result<(), String> {
        let confirmed = self.change(revision, |tx, s| {
            let done = self.discard_session(tx, s, id)?;
            if let Some(p) = s.providers.iter_mut().find(|p| p.id == id) {
                p.last_test = None
            }
            Ok(done)
        })?;
        report_revocation(confirmed)
    }

    pub fn remove(&self, revision: u64, id: &str) -> Result<(), String> {
        let confirmed = self.change(revision, |tx, s| {
            let done = self.discard_session(tx, s, id)?;
            tx.execute("DELETE FROM ai_accounts WHERE id=?1", [id])
                .map_err(sql_error)?;
            forget_bindings(s, id);
            s.providers.retain(|p| p.id != id);
            Ok(done)
        })?;
        self.usage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        report_revocation(confirmed)
    }

    /// A local bulk wipe preserves configured models, endpoints and bindings.
    pub fn clear_credentials(&self, revision: u64) -> Result<(), String> {
        self.change(revision, |tx, s| {
            self.vault.clear_ai_in(tx)?;
            tx.execute("DELETE FROM ai_accounts", [])
                .map_err(sql_error)?;
            for p in &mut s.providers {
                p.last_test = None
            }
            Ok(())
        })
    }
}
fn report_revocation(confirmed: bool) -> Result<(), String> {
    if confirmed {
        Ok(())
    } else {
        Err("Signed out locally. Remote revocation was not confirmed. Disconnect Dr. DJ Pro in ChatGPT Settings.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn serialized(access: &str, expires: u64, scope: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"access_token":access,"refresh_token":"mock-renew","expires_at":expires,"scope":scope,"nonce":"mock-nonce"})).unwrap()
    }
    #[test]
    fn rotation_is_transactional_and_does_not_repeat_another_sessions_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let r =
            AiRegistry::from_vault(Vault::session_for_test(dir.path().join("storage.db")).unwrap())
                .unwrap();
        let scope = "chatgpt.tokens.use.direct";
        r.vault
            .with_transaction(|tx| {
                tx.execute("INSERT INTO ai_accounts VALUES('p','client','subject')", [])
                    .map_err(sql_error)?;
                let expired = serialized("expired", 0, scope);
                let new = r
                    .subscription_key(tx, "p", &expired, None, |a, _| {
                        assert_eq!(a.client, "client");
                        tokens(&serialized("renewed", u64::MAX, scope))
                    })?
                    .unwrap();
                assert_eq!(new.as_str(), "renewed");
                let saved = r.vault.retrieve_in(tx, "ai:p")?.unwrap();
                let current = r
                    .subscription_key(tx, "p", &saved, Some("expired"), |_, _| {
                        panic!("rotated twice")
                    })?
                    .unwrap();
                assert_eq!(current.as_str(), "renewed");
                // A successful OAuth rotation may withdraw plan permission. Commit
                // the replacement token and its new scope, then refuse inference.
                assert!(r
                    .subscription_key(tx, "p", &saved, Some("renewed"), |_, _| tokens(
                        &serialized("identity-only", u64::MAX, "openid")
                    ))?
                    .is_none());
                Ok(())
            })
            .unwrap();
        assert!(!r.plan_enabled("p").unwrap());
        let protected = r.vault.retrieve("ai:p").unwrap().unwrap();
        assert_eq!(tokens(&protected).unwrap().access_token, "identity-only");
    }
}
