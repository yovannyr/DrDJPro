//! Durable, disposable analysis cached only for the current track, source and algorithm.
use super::{Command, Store, Worker, APPLICATION_ID, SCHEMA_VERSION};
use crate::local_harmony::{LocalKeyProfile, ALGORITHM_VERSION};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const MAX_PROFILE_BYTES: usize = 512 * 1024;

pub(super) fn schema(c: &Connection) -> Result<(), String> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS track_local_keys (
            track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
            algorithm_version INTEGER NOT NULL CHECK (algorithm_version BETWEEN 1 AND 65535),
            source_sha256 BLOB NOT NULL CHECK (length(source_sha256) = 32),
            profile BLOB NOT NULL CHECK (length(profile) BETWEEN 1 AND 524288)
        ) STRICT;",
    )
    .map_err(|e| e.to_string())
}

fn lookup(
    c: &Connection,
    id: i32,
    path: &Path,
    digest: &[u8; 32],
) -> Result<Option<LocalKeyProfile>, String> {
    const SQL: &str = "SELECT k.profile FROM track_local_keys k \
                       JOIN tracks t ON t.id = k.track_id \
                       WHERE t.id = ?1 AND t.path_key = ?2 \
                       AND k.source_sha256 = ?3 AND k.algorithm_version = ?4 \
                       AND length(k.profile) <= ?5 AND length(k.profile) >= 1 \
                       AND typeof(k.profile) = 'blob'";

    let path_key = crate::library::identity(path);
    let result = c
        .query_row(
            SQL,
            params![
                id,
                path_key.to_str().ok_or("Invalid track path")?,
                digest.as_slice(),
                ALGORITHM_VERSION,
                MAX_PROFILE_BYTES as i64
            ],
            |row| {
                let bytes = row.get_ref(0)?.as_blob()?;
                Ok(serde_json::from_slice::<LocalKeyProfile>(bytes)
                    .ok()
                    .filter(|profile| {
                        profile.source_sha256 == *digest && profile.validate().is_ok()
                    }))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(result.flatten())
}

fn save(c: &Connection, id: i32, path: &Path, profile: &LocalKeyProfile) -> Result<(), String> {
    profile.validate()?;
    let path_key = crate::library::identity(path);
    let profile_bytes = serde_json::to_vec(profile).map_err(|e| e.to_string())?;
    if profile_bytes.len() > MAX_PROFILE_BYTES {
        return Err("Profile exceeds maximum size".into());
    }

    let affected = c
        .execute(
            "INSERT INTO track_local_keys(track_id, algorithm_version, source_sha256, profile) \
         SELECT id, ?3, ?4, ?5 \
         FROM tracks \
         WHERE id = ?1 AND path_key = ?2 \
           AND (SELECT user_version FROM pragma_user_version) = ?6 \
           AND (SELECT application_id FROM pragma_application_id) = ?7 \
         ON CONFLICT(track_id) DO UPDATE SET \
           algorithm_version = EXCLUDED.algorithm_version, \
           source_sha256 = EXCLUDED.source_sha256, \
           profile = EXCLUDED.profile",
            params![
                id,
                path_key.to_str().ok_or("Invalid track path")?,
                ALGORITHM_VERSION,
                profile.source_sha256.as_slice(),
                profile_bytes,
                SCHEMA_VERSION,
                APPLICATION_ID
            ],
        )
        .map_err(|e| e.to_string())?;

    if affected != 1 {
        return Err("Track or library schema changed before local keys could be saved".into());
    }

    Ok(())
}

#[derive(Clone)]
pub(crate) struct LocalKeyCache {
    tx: mpsc::SyncSender<Command>,
}

impl Worker {
    pub(crate) fn local_key_cache(&self) -> LocalKeyCache {
        LocalKeyCache {
            tx: self.tx.clone(),
        }
    }
}

pub(super) enum LocalKeyRequest {
    Lookup {
        id: i32,
        path: PathBuf,
        digest: [u8; 32],
        reply: mpsc::Sender<Result<Option<LocalKeyProfile>, String>>,
    },
    Save {
        id: i32,
        path: PathBuf,
        profile: LocalKeyProfile,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

impl LocalKeyRequest {
    pub(super) fn run(self, store: &Store) {
        match self {
            Self::Lookup {
                id,
                path,
                digest,
                reply,
            } => {
                let result = lookup(&store.connection, id, &path, &digest);
                let _ = reply.send(result);
            }
            Self::Save {
                id,
                path,
                profile,
                reply,
            } => {
                let result = save(&store.connection, id, &path, &profile);
                let _ = reply.send(result);
            }
        }
    }
}

impl LocalKeyCache {
    pub(crate) fn lookup(
        &self,
        id: i32,
        path: &Path,
        digest: &[u8; 32],
    ) -> Result<Option<LocalKeyProfile>, String> {
        let (tx, rx) = mpsc::channel();
        let req = LocalKeyRequest::Lookup {
            id,
            path: path.to_path_buf(),
            digest: *digest,
            reply: tx,
        };
        self.tx
            .try_send(Command::LocalKey(Box::new(req)))
            .map_err(|e| format!("Local key cache unavailable: {e}"))?;
        rx.recv().map_err(|error| error.to_string())?
    }

    pub(crate) fn save(
        &self,
        id: i32,
        path: &Path,
        profile: &LocalKeyProfile,
    ) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        profile.validate()?;
        let req = LocalKeyRequest::Save {
            id,
            path: path.to_path_buf(),
            profile: profile.clone(),
            reply: tx,
        };
        self.tx
            .try_send(Command::LocalKey(Box::new(req)))
            .map_err(|e| format!("Local key cache unavailable: {e}"))?;
        rx.recv().map_err(|error| error.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_harmony::KeySection;
    use crate::track_metadata::MusicLabels;

    fn sample() -> LocalKeyProfile {
        LocalKeyProfile {
            algorithm_version: ALGORITHM_VERSION,
            source_sha256: [2; 32],
            duration_ms: 4_000,
            segments: vec![KeySection {
                start_ms: 0,
                end_ms: 4_000,
                key: Some("8B".into()),
                confidence: 90,
            }],
        }
    }

    #[test]
    fn a06_cache_migrates_v21_preserves_manual_labels_and_reopens_through_worker() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.sqlite3");
        let path = crate::test_support::fixture();
        let other = dir.path().join("different.mp3");
        let (mut store, mut library, _) = Store::open(db.clone()).unwrap();
        library.insert([path.clone()]);
        let id = library.track_id_for_path(&path).unwrap();
        let labels = MusicLabels::parse("12A", "9").unwrap();
        library.set_music_labels(id, labels).unwrap();
        store.save_delta(&library.changes()).unwrap();
        store
            .connection
            .execute_batch("DROP TABLE track_local_keys; PRAGMA user_version=21;")
            .unwrap();
        drop(store);

        let (mut store, library, _) = Store::open(db.clone()).unwrap();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            library.track_by_id(id).unwrap().metadata.music_labels,
            labels
        );
        let profile = sample();
        assert!(lookup(&store.connection, id, &path, &profile.source_sha256)
            .unwrap()
            .is_none());
        save(&store.connection, id, &path, &profile).unwrap();
        store.save(&library.catalog()).unwrap();
        drop(store);

        let worker = Worker::start(db).unwrap();
        let crate::library_store::Event::Loaded(Ok(loaded)) = worker.events.recv().unwrap() else {
            panic!("catalog not loaded")
        };
        assert_eq!(
            loaded
                .library
                .track_by_id(id)
                .unwrap()
                .metadata
                .music_labels,
            labels
        );
        let cache = worker.local_key_cache();
        assert_eq!(
            cache.lookup(id, &path, &[2; 32]).unwrap(),
            Some(profile.clone())
        );
        assert!(cache.lookup(id, &other, &[2; 32]).unwrap().is_none());
        assert!(cache.lookup(id, &path, &[3; 32]).unwrap().is_none());
        assert!(cache.save(id, &other, &profile).is_err());
        assert!(cache.save(id + 1, &path, &profile).is_err());
        let mut invalid = profile.clone();
        invalid.segments[0].confidence = 255;
        assert!(cache.save(id, &path, &invalid).is_err());
        assert_eq!(cache.lookup(id, &path, &[2; 32]).unwrap(), Some(profile));
        worker.close(None).unwrap();
    }

    #[test]
    fn a06_cache_ignores_invalid_mismatched_or_oversized_data_and_cascades_on_track_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let path = crate::test_support::fixture();
        let (mut store, mut library, _) = Store::open(dir.path().join("cache.sqlite3")).unwrap();
        library.insert([path.clone()]);
        let id = library.track_id_for_path(&path).unwrap();
        store.save_delta(&library.changes()).unwrap();
        let db = &store.connection;
        let profile = sample();
        save(db, id, &path, &profile).unwrap();
        let missed = || {
            lookup(db, id, &path, &profile.source_sha256)
                .unwrap()
                .is_none()
        };

        db.execute(
            "UPDATE track_local_keys SET algorithm_version=algorithm_version+1",
            [],
        )
        .unwrap();
        assert!(missed());
        save(db, id, &path, &profile).unwrap();
        db.execute(
            "UPDATE track_local_keys SET profile=?1",
            [b"{malformed".as_slice()],
        )
        .unwrap();
        assert!(missed());
        let mut bad = profile.clone();
        bad.source_sha256 = [3; 32];
        db.execute(
            "UPDATE track_local_keys SET profile=?1",
            [serde_json::to_vec(&bad).unwrap()],
        )
        .unwrap();
        assert!(missed());
        let mut bad = profile.clone();
        bad.segments[0].start_ms = 1;
        db.execute(
            "UPDATE track_local_keys SET profile=?1",
            [serde_json::to_vec(&bad).unwrap()],
        )
        .unwrap();
        assert!(missed());
        db.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE track_local_keys SET profile=zeroblob(524289); PRAGMA ignore_check_constraints=OFF;").unwrap();
        assert!(missed());

        save(db, id, &path, &profile).unwrap();
        db.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(save(db, id, &path, &profile).is_err());
        db.pragma_update(None, "user_version", SCHEMA_VERSION)
            .unwrap();
        db.execute("DELETE FROM tracks WHERE id=?1", [id]).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM track_local_keys", [], |row| row
                .get::<_, i32>(0))
                .unwrap(),
            0
        );
    }
}
