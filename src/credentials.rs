//! Versioned credential storage. Only authenticated ciphertext is written to SQLite.
//! The vault's random AES key is held in the user's OS credential store. When
//! it is unavailable, a process-local key and a memory-only SQLite TEMP table
//! provide session credentials. A non-secret marker coordinates replacement
//! and deletion between simultaneous instances in either mode.
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use zeroize::Zeroizing;

/// Rows from version 1 used a discarded per-process key.
pub const CREDENTIAL_FORMAT_VERSION: u32 = 2;
const SESSION_MARKER: u32 = 3;
type EncryptionKey = Zeroizing<[u8; 32]>;

fn random_key() -> Result<EncryptionKey, String> {
    let mut key = Zeroizing::new([0; 32]);
    getrandom::fill(&mut key[..]).map_err(|_| "Secure randomness is unavailable")?;
    Ok(key)
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn os_key(id: &str, allow_create: bool) -> Option<EncryptionKey> {
    let entry = keyring::Entry::new("DrDJPro.Vault", id).ok()?;
    match entry.get_secret() {
        Ok(bytes) => {
            let bytes = Zeroizing::new(bytes);
            let mut key = Zeroizing::new([0; 32]);
            if bytes.len() != key.len() {
                return None;
            }
            key.copy_from_slice(&bytes);
            Some(key)
        }
        Err(keyring::Error::NoEntry) if allow_create => {
            let key = random_key().ok()?;
            entry.set_secret(&key[..]).ok()?;
            Some(key)
        }
        _ => None,
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn os_key(_: &str, _: bool) -> Option<EncryptionKey> {
    None
}

fn encrypt(key: &[u8; 32], service: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0; 12];
    getrandom::fill(&mut nonce).map_err(|_| "Secure randomness is unavailable")?;
    let ciphertext = Aes256Gcm::new(key.into())
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: data,
                aad: service.as_bytes(),
            },
        )
        .map_err(|_| "Credential encryption failed")?;
    let mut result = Vec::with_capacity(nonce.len() + ciphertext.len());
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

fn decrypt(key: &[u8; 32], service: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < 12 + 16 {
        return Err("Invalid credential ciphertext".into());
    }
    let (nonce, ciphertext) = bytes.split_at(12);
    Aes256Gcm::new(key.into())
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: service.as_bytes(),
            },
        )
        .map_err(|_| "Credential authentication failed".into())
}

fn database_error(_: rusqlite::Error) -> String {
    "Credential database unavailable or invalid".into()
}

/// Serialized SQLite connection, private session table, and OS-backed or
/// process-local encryption key. Open once per app session on a worker thread.
pub struct Vault {
    db: Mutex<Connection>,
    key: EncryptionKey,
    persistent: bool,
    discarded: AtomicUsize,
}

impl Vault {
    #[cfg(test)]
    pub(crate) fn session_for_test(path: PathBuf) -> Result<Self, String> {
        Self::open_with(path, |_, _| None)
    }

    /// Create or open the vault. A locked or unavailable key store never causes
    /// existing protected rows to be erased or an OS key to be replaced.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        Self::open_with(path.into(), os_key)
    }

    fn open_with(
        path: PathBuf,
        obtain: impl FnOnce(&str, bool) -> Option<EncryptionKey>,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| "Credential directory unavailable")?;
        }
        let mut db = Connection::open(path).map_err(database_error)?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(database_error)?;
        db.execute_batch(
            "PRAGMA temp_store=MEMORY;
            PRAGMA secure_delete=ON;
            PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS credentials_v1 (
                service TEXT PRIMARY KEY, version INTEGER NOT NULL,
                encrypted_blob BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS credential_vault_id (
                slot INTEGER PRIMARY KEY CHECK(slot=1), id TEXT NOT NULL
            );
            INSERT OR IGNORE INTO credential_vault_id VALUES(1,lower(hex(randomblob(16))));
            CREATE TEMP TABLE session_credentials (
                service TEXT PRIMARY KEY, marker BLOB NOT NULL, payload BLOB NOT NULL
            );",
        )
        .map_err(database_error)?;
        // Concurrent creators must choose a single OS key. The stable identifier
        // is already committed, so it survives interruption after key creation.
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let (id, can_create): (String,bool)=tx.query_row(
            "SELECT id, NOT EXISTS(SELECT 1 FROM credentials_v1 WHERE version=2) FROM credential_vault_id WHERE slot=1",
            [], |row| Ok((row.get(0)?,row.get(1)?))).map_err(database_error)?;
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid vault identifier".into());
        }
        let os = obtain(&id, can_create);
        let persistent = os.is_some();
        let key = match os {
            Some(key) => key,
            None => random_key()?,
        };
        let discarded = tx
            .execute("DELETE FROM credentials_v1 WHERE version=1", [])
            .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(Self {
            db: Mutex::new(db),
            key,
            persistent,
            discarded: AtomicUsize::new(discarded),
        })
    }

    /// Whether newly stored credentials survive this application session.
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Number of discarded legacy rows, delivered once for the UI notice.
    pub fn take_discarded_legacy_count(&self) -> usize {
        self.discarded.swap(0, Ordering::Relaxed)
    }

    /// Atomically edit non-secret settings and their credentials in the same DB.
    /// The caller must use this transaction's connection for both operations.
    pub fn with_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut db = self.db.lock().map_err(|_| "Credential lock unavailable")?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let value = operation(&tx)?;
        tx.commit().map_err(database_error)?;
        Ok(value)
    }

    /// Replace a secret inside a transaction opened by this vault.
    pub fn store_in(&self, tx: &Transaction<'_>, service: &str, data: &[u8]) -> Result<(), String> {
        let protected = encrypt(&self.key, service, data)?;
        if self.persistent {
            tx.execute(
                "INSERT OR REPLACE INTO credentials_v1 VALUES(?1,?2,?3)",
                params![service, CREDENTIAL_FORMAT_VERSION, protected],
            )
            .map_err(database_error)?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO session_credentials VALUES(?1,randomblob(16),?2)",
                params![service, protected],
            )
            .map_err(database_error)?;
            tx.execute(
                "INSERT OR REPLACE INTO credentials_v1
                 SELECT service,?2,marker FROM session_credentials WHERE service=?1",
                params![service, SESSION_MARKER],
            )
            .map_err(database_error)?;
        }
        Ok(())
    }

    /// Store a single credential.
    pub fn store(&self, service: &str, data: &[u8]) -> Result<(), String> {
        self.with_transaction(|tx| self.store_in(tx, service, data))
    }

    /// Retrieve a credential. An inaccessible persistent secret is never replaced
    /// by an empty value. A marker from another session has no usable secret here.
    pub fn retrieve(&self, service: &str) -> Result<Option<Vec<u8>>, String> {
        let db = self.db.lock().map_err(|_| "Credential lock unavailable")?;
        self.read_from(&db, service)
    }

    /// Check whether this session can access a credential without decrypting it
    /// just to render a status indicator.
    pub(crate) fn contains_in(&self, tx: &Transaction<'_>, service: &str) -> Result<bool, String> {
        tx.query_row("SELECT EXISTS(SELECT 1 FROM credentials_v1 c WHERE c.service=?1 AND
            ((c.version=?2 AND ?3) OR (c.version=?4 AND EXISTS
            (SELECT 1 FROM session_credentials s WHERE s.service=c.service AND s.marker=c.encrypted_blob))))",
            params![service,CREDENTIAL_FORMAT_VERSION,self.persistent,SESSION_MARKER],|r|r.get(0)).map_err(database_error)
    }

    /// Read a secret inside an ongoing registry transaction.
    pub fn retrieve_in(
        &self,
        tx: &Transaction<'_>,
        service: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        self.read_from(tx, service)
    }

    fn read_from(&self, db: &Connection, service: &str) -> Result<Option<Vec<u8>>, String> {
        let sql =
            "SELECT c.version, CASE WHEN c.version=?2 THEN s.payload ELSE c.encrypted_blob END
                   FROM credentials_v1 c LEFT JOIN session_credentials s
                     ON c.service=s.service AND c.encrypted_blob=s.marker
                   WHERE c.service=?1";
        let row: Option<(u32, Option<Vec<u8>>)> = db
            .query_row(sql, params![service, SESSION_MARKER], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(database_error)?;
        let Some((version, bytes)) = row else {
            return Ok(None);
        };
        match version {
            CREDENTIAL_FORMAT_VERSION if !self.persistent => {
                Err("Unlock the OS credential store to access the saved key".into())
            }
            CREDENTIAL_FORMAT_VERSION | SESSION_MARKER => {
                bytes.map(|b| decrypt(&self.key, service, &b)).transpose()
            }
            _ => Err("Unsupported credential format version".into()),
        }
    }

    /// Delete a credential in a transaction opened by this vault.
    pub fn delete_in(&self, tx: &Transaction<'_>, service: &str) -> Result<(), String> {
        tx.execute("DELETE FROM credentials_v1 WHERE service=?1", [service])
            .map_err(database_error)?;
        tx.execute(
            "DELETE FROM session_credentials WHERE service=?1",
            [service],
        )
        .map_err(database_error)?;
        Ok(())
    }

    /// Delete a credential even when the OS credential store is unavailable.
    pub fn delete(&self, service: &str) -> Result<(), String> {
        self.with_transaction(|tx| self.delete_in(tx, service))
    }

    /// Names of credentials stored persistently or visible in this session.
    pub fn list_services(&self) -> Result<Vec<String>, String> {
        let db = self.db.lock().map_err(|_| "Credential lock unavailable")?;
        let mut stmt=db.prepare("SELECT c.service FROM credentials_v1 c WHERE c.version!=?1 OR EXISTS (
            SELECT 1 FROM session_credentials s WHERE s.service=c.service AND s.marker=c.encrypted_blob
        ) ORDER BY c.service").map_err(database_error)?;
        let rows = stmt
            .query_map([SESSION_MARKER], |r| r.get(0))
            .map_err(database_error)?;
        rows.collect::<Result<_, _>>().map_err(database_error)
    }

    /// Delete only AI credentials in a transaction opened by this vault.
    pub fn clear_ai_in(&self, tx: &Transaction<'_>) -> Result<(), String> {
        tx.execute_batch(
            "DELETE FROM credentials_v1 WHERE service GLOB 'ai:*';
            DELETE FROM session_credentials WHERE service GLOB 'ai:*';",
        )
        .map_err(database_error)
    }

    /// Delete only credentials belonging to AI providers.
    pub fn clear_ai(&self) -> Result<(), String> {
        self.with_transaction(|tx| self.clear_ai_in(tx))
    }

    /// Clear all credentials, including non-AI credentials.
    pub fn clear(&self) -> Result<(), String> {
        self.with_transaction(|tx| {
            tx.execute_batch("DELETE FROM credentials_v1; DELETE FROM session_credentials;")
                .map_err(database_error)
        })
    }
}

/// Thread-safe credential manager shared across the application.
pub struct Manager {
    vault: Vault,
}

impl Manager {
    /// Create a new credential manager backed by a SQLite database at `db_path`.
    pub fn new(db_path: impl Into<PathBuf>) -> Result<Self, String> {
        let vault = Vault::open(db_path)?;
        Ok(Self { vault })
    }

    /// Store an OAuth token or other credential.
    pub fn store(&self, service: &str, data: &[u8]) -> Result<(), String> {
        self.vault.store(service, data)
    }

    /// Retrieve and decrypt a credential.
    pub fn retrieve(&self, service: &str) -> Result<Option<Vec<u8>>, String> {
        self.vault.retrieve(service)
    }

    /// Remove a stored credential.
    pub fn delete(&self, service: &str) -> Result<(), String> {
        self.vault.delete(service)
    }

    /// List registered service names.
    pub fn list_services(&self) -> Result<Vec<String>, String> {
        self.vault.list_services()
    }

    /// Clear all stored credentials.
    pub fn clear(&self) -> Result<(), String> {
        self.vault.clear()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> (Manager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.db");
        let mgr = Manager {
            vault: Vault::open_with(path, |_, _| None).unwrap(),
        };
        (mgr, dir)
    }

    #[test]
    fn roundtrip_store_and_retrieve() {
        let (mgr, _dir) = temp_vault();
        let data = b"{\"access_token\":\"abc123\"}";
        mgr.store("spotify", data).unwrap();

        let result = mgr.retrieve("spotify").unwrap().unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn retrieve_missing_service_returns_none() {
        let (mgr, _dir) = temp_vault();
        let result = mgr.retrieve("spotify").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn delete_removes_credential() {
        let (mgr, _dir) = temp_vault();
        mgr.store("spotify", b"token").unwrap();
        mgr.delete("spotify").unwrap();
        assert!(mgr.retrieve("spotify").unwrap().is_none());
    }

    #[test]
    fn multiple_services_independent() {
        let (mgr, _dir) = temp_vault();
        mgr.store("spotify", b"spotify-token").unwrap();
        mgr.store("tidal", b"tidal-token").unwrap();

        assert_eq!(mgr.retrieve("spotify").unwrap().unwrap(), b"spotify-token");
        assert_eq!(mgr.retrieve("tidal").unwrap().unwrap(), b"tidal-token");

        let services = mgr.list_services().unwrap();
        assert_eq!(services, vec!["spotify", "tidal"]);
    }

    #[test]
    fn clear_removes_all() {
        let (mgr, _dir) = temp_vault();
        mgr.store("spotify", b"token").unwrap();
        mgr.store("tidal", b"token").unwrap();
        mgr.clear().unwrap();
        assert!(mgr.retrieve("spotify").unwrap().is_none());
        assert!(mgr.retrieve("tidal").unwrap().is_none());
        assert!(mgr.list_services().unwrap().is_empty());
    }

    #[test]
    fn vault_open_nonexistent_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new_vault.db");
        let vault = Vault::open_with(path.clone(), |_, _| None).unwrap();
        assert!(path.exists());
        vault.clear().unwrap();
    }

    #[test]
    fn update_existing_credential() {
        let (mgr, _dir) = temp_vault();
        mgr.store("spotify", b"token1").unwrap();
        mgr.store("spotify", b"token2").unwrap();
        assert_eq!(mgr.retrieve("spotify").unwrap().unwrap(), b"token2");
    }

    #[test]
    fn large_credential_payload() {
        let (mgr, _dir) = temp_vault();
        let large = vec![b'x'; 10_000];
        mgr.store("large", &large).unwrap();
        assert_eq!(mgr.retrieve("large").unwrap().unwrap(), large);
    }

    #[test]
    fn tampering_and_swapping_service_names_fail_authentication() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(dir.path().join("db"), |_, _| random_key().ok()).unwrap();
        vault.store("ai:a", b"protected test value").unwrap();
        vault.with_transaction(|tx| {
            tx.execute("INSERT INTO credentials_v1 SELECT 'ai:b',version,encrypted_blob FROM credentials_v1 WHERE service='ai:a'", []).unwrap();
            tx.execute("UPDATE credentials_v1 SET encrypted_blob=zeroblob(28) WHERE service='ai:a'", []).unwrap();
            Ok(())
        }).unwrap();
        for name in ["ai:a", "ai:b"] {
            assert!(vault.retrieve(name).is_err());
        }
    }

    #[test]
    fn unavailable_store_preserves_existing_keys_and_discards_session_values_on_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let shared_key = random_key().unwrap();
        let persistent = Vault::open_with(path.clone(), |_, _| Some(shared_key.clone())).unwrap();
        persistent.store("music:unchanged", b"persistent").unwrap();
        persistent.store("ai:one", b"old key").unwrap();
        let locked = Vault::open_with(path.clone(), |_, can_create| {
            assert!(!can_create);
            None
        })
        .unwrap();
        assert!(!locked.is_persistent());
        assert!(locked.retrieve("music:unchanged").is_err());
        locked.store("ai:one", b"temporary key").unwrap();
        assert_eq!(
            locked.retrieve("ai:one").unwrap().unwrap(),
            b"temporary key"
        );
        assert!(persistent.retrieve("ai:one").unwrap().is_none());
        persistent.delete("ai:one").unwrap();
        assert!(locked.retrieve("ai:one").unwrap().is_none());
        locked.store("ai:one", b"temporary key").unwrap();
        drop(locked);
        drop(persistent);
        let again = Vault::open_with(path, |_, _| Some(shared_key)).unwrap();
        assert!(again.is_persistent());
        assert_eq!(again.list_services().unwrap(), vec!["music:unchanged"]);
        assert_eq!(
            again.retrieve("music:unchanged").unwrap().unwrap(),
            b"persistent"
        );
        assert!(again.retrieve("ai:one").unwrap().is_none());
    }

    #[test]
    fn credentials_roll_back_with_provider_edits_in_both_storage_modes() {
        for persistent in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let vault = Vault::open_with(dir.path().join("db"), |_, _| {
                persistent.then(|| random_key().unwrap())
            })
            .unwrap();
            vault.store("ai:test", b"first").unwrap();
            let result: Result<(), String> = vault.with_transaction(|tx| {
                vault.store_in(tx, "ai:test", b"new").unwrap();
                assert_eq!(vault.retrieve_in(tx, "ai:test").unwrap().unwrap(), b"new");
                Err("abort".into())
            });
            assert!(result.is_err());
            assert_eq!(vault.retrieve("ai:test").unwrap().unwrap(), b"first");
            vault.store("stream:keep", b"music").unwrap();
            vault.clear_ai().unwrap();
            assert_eq!(vault.list_services().unwrap(), vec!["stream:keep"]);
        }
    }

    #[test]
    fn sqlite_files_never_contain_the_canary_in_either_mode() {
        let canary = b"drdj-vault-plain-secret-canary-34c9263";
        for persistent in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let vault = Vault::open_with(dir.path().join("db"), |_, _| {
                persistent.then(|| random_key().unwrap())
            })
            .unwrap();
            vault.store("ai:canary", canary).unwrap();
            assert_eq!(vault.retrieve("ai:canary").unwrap().unwrap(), canary);
            for file in std::fs::read_dir(dir.path()).unwrap() {
                let data = std::fs::read(file.unwrap().path()).unwrap();
                assert!(!data.windows(canary.len()).any(|part| part == canary));
            }
        }
    }

    #[test]
    fn obsolete_session_rows_are_discarded_and_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let vault = Vault::open_with(path.clone(), |_, _| None).unwrap();
        vault
            .with_transaction(|tx| {
                tx.execute(
                    "INSERT INTO credentials_v1 VALUES('old',1,randomblob(40))",
                    [],
                )
                .unwrap();
                tx.execute(
                    "INSERT INTO credentials_v1 VALUES('future',99,randomblob(40))",
                    [],
                )
                .unwrap();
                Ok(())
            })
            .unwrap();
        drop(vault);
        let repaired = Vault::open_with(path.clone(), |_, _| None).unwrap();
        assert_eq!(repaired.take_discarded_legacy_count(), 1);
        assert_eq!(repaired.take_discarded_legacy_count(), 0);
        assert!(repaired.retrieve("old").unwrap().is_none());
        assert!(repaired.retrieve("future").is_err());
        let reopened = Vault::open_with(path, |_, _| None).unwrap();
        assert_eq!(reopened.take_discarded_legacy_count(), 0);
    }

    #[test]
    fn simultaneous_initializations_observe_one_vault_identity() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let rendezvous = Arc::new(Barrier::new(2));
        let keys = Arc::new(Mutex::new(std::collections::HashMap::new()));
        std::thread::scope(|scope| {
            for service in ["ai:first", "ai:second"] {
                let (path, keys, gate) = (path.clone(), keys.clone(), rendezvous.clone());
                scope.spawn(move || {
                    gate.wait();
                    let vault = Vault::open_with(path, |id, _| {
                        Some(
                            keys.lock()
                                .unwrap()
                                .entry(id.to_owned())
                                .or_insert_with(|| random_key().unwrap())
                                .clone(),
                        )
                    })
                    .unwrap();
                    vault.store(service, service.as_bytes()).unwrap();
                });
            }
        });
        let keys = keys.lock().unwrap();
        assert_eq!(keys.len(), 1);
        let vault = Vault::open_with(path, |id, _| keys.get(id).cloned()).unwrap();
        for service in ["ai:first", "ai:second"] {
            assert_eq!(
                vault.retrieve(service).unwrap().unwrap(),
                service.as_bytes()
            );
        }
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[ignore = "requires an unlocked native credential store; run explicitly"]
    fn native_restart_uses_os_key_in_independent_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.sqlite");
        let vault = Vault::open(&path).unwrap();
        assert!(vault.is_persistent(), "OS credential store unavailable");
        let id: String = vault
            .with_transaction(|tx| {
                tx.query_row("SELECT id FROM credential_vault_id", [], |r| r.get(0))
                    .map_err(database_error)
            })
            .unwrap();
        struct RemoveTestKey(keyring::Entry);
        impl Drop for RemoveTestKey {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        let _cleanup = RemoveTestKey(keyring::Entry::new("DrDJPro.Vault", &id).unwrap());
        vault
            .store("ai:restart-test", b"drdj-native-restart-test-only")
            .unwrap();
        drop(vault);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "credentials::tests::native_restart_child",
                "--ignored",
            ])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(Vault::open(path)
            .unwrap()
            .retrieve("ai:restart-test")
            .unwrap()
            .is_none());
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[ignore = "child for the native restart qualification"]
    fn native_restart_child() {
        let path = std::path::Path::new("native.sqlite");
        assert!(path.is_file());
        let vault = Vault::open(path).unwrap();
        assert!(vault.is_persistent());
        assert_eq!(
            vault.retrieve("ai:restart-test").unwrap().unwrap(),
            b"drdj-native-restart-test-only"
        );
        vault.delete("ai:restart-test").unwrap();
    }
}
