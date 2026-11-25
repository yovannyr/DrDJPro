//! User-confirmed, source-absolute chord intervals. Missing rows mean unconfirmed.
use super::{Command, Store, Worker, APPLICATION_ID, SCHEMA_VERSION};
use crate::chord_passage::{canonical_chord, ChordEvent};
use rusqlite::{params, Connection, TransactionBehavior};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

const MAX_CONFIRMED: i64 = 4096;
const MAX_TIME_MS: u64 = 86_400_000;

fn err(error: rusqlite::Error) -> String {
    error.to_string()
}

pub(super) fn schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS track_chord_confirmations(
        track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        source_sha256 BLOB NOT NULL CHECK(length(source_sha256)=32),
        start_ms INTEGER NOT NULL CHECK(start_ms>=0 AND start_ms<86400000),
        end_ms INTEGER NOT NULL CHECK(end_ms>start_ms AND end_ms<=86400000),
        chord TEXT CHECK(chord IS NULL OR length(CAST(chord AS BLOB)) BETWEEN 5 AND 6),
        PRIMARY KEY(track_id,source_sha256,start_ms)
    ) STRICT, WITHOUT ROWID;",
    )
    .map_err(err)
}

fn validate_range(start: u64, end: u64) -> Result<(), String> {
    if start >= end || end > MAX_TIME_MS {
        return Err("Invalid source interval for a chord confirmation".into());
    }
    Ok(())
}
fn validate_chord(chord: Option<&str>) -> Result<(), String> {
    if let Some(raw) = chord {
        if !(5..=6).contains(&raw.len()) || canonical_chord(raw).as_deref() != Some(raw) {
            return Err("Invalid confirmed chord".into());
        }
    }
    Ok(())
}

fn identity(c: &Connection, s: &Subject) -> Result<(), String> {
    let key = crate::library::identity(&s.path);
    let matches: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tracks WHERE id=?1 AND path_key=?2)
        AND (SELECT user_version FROM pragma_user_version)=?3
        AND (SELECT application_id FROM pragma_application_id)=?4",
            params![
                s.id,
                key.to_str().ok_or("Invalid track path")?,
                SCHEMA_VERSION,
                APPLICATION_ID
            ],
            |row| row.get(0),
        )
        .map_err(err)?;
    if !matches {
        return Err("Track or catalog changed. Reopen this chord passage.".into());
    }
    Ok(())
}

/// Return the source intervals in full. A write uses the edge rows to retain their uncovered parts.
fn overlapping(
    db: &Connection,
    sub: &Subject,
    start: u64,
    end: u64,
) -> Result<Vec<ChordEvent>, String> {
    let mut stmt=db.prepare("SELECT start_ms,end_ms,
        CASE WHEN chord IS NULL OR length(CAST(chord AS BLOB)) BETWEEN 5 AND 6 THEN chord ELSE '!' END
        FROM track_chord_confirmations WHERE track_id=?1 AND source_sha256=?2
        AND start_ms<?4 AND end_ms>?3 ORDER BY start_ms LIMIT ?5").map_err(err)?;
    let mut rows = stmt
        .query(params![
            sub.id,
            sub.digest.as_slice(),
            start as i64,
            end as i64,
            MAX_CONFIRMED + 1
        ])
        .map_err(err)?;
    let mut events = Vec::new();
    let mut last_end = 0;
    while let Some(row) = rows.next().map_err(err)? {
        let start_ms = u64::try_from(row.get::<_, i64>(0).map_err(err)?)
            .map_err(|_| "Invalid negative source start")?;
        let end_ms = u64::try_from(row.get::<_, i64>(1).map_err(err)?)
            .map_err(|_| "Invalid negative source end")?;
        validate_range(start_ms, end_ms)?;
        let chord: Option<String> = row.get(2).map_err(err)?;
        validate_chord(chord.as_deref())?;
        if start_ms < last_end || events.len() as i64 >= MAX_CONFIRMED {
            return Err("Invalid overlapping or excessive chord confirmations".into());
        }
        last_end = end_ms;
        let confidence = if chord.is_some() { 100 } else { 0 };
        events.push(ChordEvent {
            start_ms,
            end_ms,
            chord,
            confidence,
        });
    }
    Ok(events)
}

fn lookup(db: &Connection, sub: &Subject, start: u64, end: u64) -> Result<Vec<ChordEvent>, String> {
    validate_range(start, end)?;
    let tx = db.unchecked_transaction().map_err(err)?;
    identity(&tx, sub)?;
    let mut events = overlapping(&tx, sub, start, end)?;
    tx.commit().map_err(err)?;
    for event in &mut events {
        event.start_ms = event.start_ms.max(start);
        event.end_ms = event.end_ms.min(end);
    }
    Ok(events)
}

fn insert(
    db: &Connection,
    s: &Subject,
    start: u64,
    end: u64,
    chord: Option<&str>,
) -> Result<(), String> {
    db.execute(
        "INSERT INTO track_chord_confirmations(track_id,source_sha256,start_ms,end_ms,chord)
        VALUES(?1,?2,?3,?4,?5)",
        params![s.id, s.digest.as_slice(), start as i64, end as i64, chord],
    )
    .map_err(err)?;
    Ok(())
}

fn correct(db: &mut Connection, subject: &Subject, event: &ChordEvent) -> Result<(), String> {
    validate_range(event.start_ms, event.end_ms)?;
    validate_chord(event.chord.as_deref())?;

    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;

    identity(&tx, subject)?;

    let prior = overlapping(&tx, subject, event.start_ms, event.end_ms)?;

    tx.execute(
        "DELETE FROM track_chord_confirmations WHERE track_id = ?1 AND source_sha256 = ?2 AND start_ms < ?4 AND end_ms > ?3",
        params![subject.id, &subject.digest[..], event.start_ms as i64, event.end_ms as i64],
    ).map_err(err)?;

    if let Some(p) = prior.first().filter(|p| p.start_ms < event.start_ms) {
        insert(&tx, subject, p.start_ms, event.start_ms, p.chord.as_deref())?;
    }

    if let Some(p) = prior.last().filter(|p| event.end_ms < p.end_ms) {
        insert(&tx, subject, event.end_ms, p.end_ms, p.chord.as_deref())?;
    }

    insert(
        &tx,
        subject,
        event.start_ms,
        event.end_ms,
        event.chord.as_deref(),
    )?;

    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM track_chord_confirmations WHERE track_id = ?1",
            params![subject.id],
            |row| row.get(0),
        )
        .map_err(err)?;

    if count > MAX_CONFIRMED {
        return Err("Limit of 4096 confirmed intervals per track reached".into());
    }

    tx.commit().map_err(err)
}

#[derive(Clone)]
pub(crate) struct ChordConfirmations {
    tx: mpsc::SyncSender<Command>,
}

pub(super) struct Subject {
    pub id: i32,
    pub path: PathBuf,
    pub digest: [u8; 32],
}

pub(super) enum ChordRequest {
    Read {
        subject: Subject,
        start_ms: u64,
        end_ms: u64,
        reply: mpsc::Sender<Result<Vec<ChordEvent>, String>>,
    },
    Write {
        subject: Subject,
        event: ChordEvent,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

impl Worker {
    pub(crate) fn chord_confirmations(&self) -> ChordConfirmations {
        ChordConfirmations {
            tx: self.tx.clone(),
        }
    }
}

impl ChordConfirmations {
    pub(crate) fn lookup(
        &self,
        id: i32,
        path: &Path,
        digest: &[u8; 32],
        start_ms: u64,
        end_ms: u64,
    ) -> Result<Vec<ChordEvent>, String> {
        let (tx, rx) = mpsc::channel();
        let subject = Subject {
            id,
            path: path.to_path_buf(),
            digest: *digest,
        };
        let req = ChordRequest::Read {
            subject,
            start_ms,
            end_ms,
            reply: tx,
        };
        self.tx
            .try_send(Command::Chord(Box::new(req)))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub(crate) fn confirm(
        &self,
        id: i32,
        path: &Path,
        digest: &[u8; 32],
        event: &ChordEvent,
    ) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        let subject = Subject {
            id,
            path: path.to_path_buf(),
            digest: *digest,
        };
        let req = ChordRequest::Write {
            subject,
            event: event.clone(),
            reply: tx,
        };
        self.tx
            .try_send(Command::Chord(Box::new(req)))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
}

impl ChordRequest {
    pub(super) fn run(self, store: &mut Store) {
        match &self {
            ChordRequest::Read {
                subject,
                start_ms,
                end_ms,
                reply,
            } => {
                let result = lookup(&store.connection, subject, *start_ms, *end_ms);
                let _ = reply.send(result);
            }
            ChordRequest::Write {
                subject,
                event,
                reply,
            } => {
                let result = correct(&mut store.connection, subject, event);
                if result.is_ok() {
                    store.backup_pending = true;
                }
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture;

    fn event(start_ms: u64, end_ms: u64, chord: Option<&str>) -> ChordEvent {
        ChordEvent {
            start_ms,
            end_ms,
            chord: chord.map(str::to_owned),
            confidence: if chord.is_some() { 100 } else { 0 },
        }
    }

    #[test]
    fn d01_confirmations_migrate_split_reopen_and_separate_source_versions() {
        let path = fixture();
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        let other = path.with_file_name("format-flac.flac");
        let (mut store, mut lib, _) = Store::open(db.clone()).unwrap();
        lib.insert([path.clone(), other.clone()]);
        let id = lib.track_id_for_path(&path).unwrap();
        let other_id = lib.track_id_for_path(&other).unwrap();
        store.save_delta(&lib.changes()).unwrap();
        store
            .connection
            .execute_batch("DROP TABLE track_chord_confirmations; PRAGMA user_version=22;")
            .unwrap();
        drop(store);
        let worker = Worker::start(db.clone()).unwrap();
        assert!(matches!(
            worker.events.recv().unwrap(),
            crate::library_store::Event::Loaded(Ok(_))
        ));
        let h = worker.chord_confirmations();
        let digest = [17; 32];
        let replacement = [18; 32];
        assert!(h.lookup(id, &path, &digest, 0, 2000).unwrap().is_empty());
        h.confirm(other_id, &other, &digest, &event(0, 900, Some("G:min")))
            .unwrap();
        h.confirm(id, &path, &digest, &event(100, 1000, Some("D:min")))
            .unwrap();
        h.confirm(id, &path, &digest, &event(300, 600, Some("A:maj")))
            .unwrap();
        h.confirm(id, &path, &digest, &event(500, 700, None))
            .unwrap();
        assert!(h
            .lookup(id, &path, &replacement, 100, 1000)
            .unwrap()
            .is_empty());
        h.confirm(id, &path, &replacement, &event(5, 900, Some("Eb:min")))
            .unwrap();
        assert!(h.confirm(id, &other, &digest, &event(5, 10, None)).is_err());
        assert!(h
            .confirm(other_id, &path, &digest, &event(5, 10, None))
            .is_err());
        assert!(h
            .confirm(id, &path, &digest, &event(490, 485, None))
            .is_err());
        assert!(h
            .confirm(id, &path, &digest, &event(200, 500, Some("D#min")))
            .is_err());
        let expected = vec![
            event(200, 300, Some("D:min")),
            event(300, 500, Some("A:maj")),
            event(500, 700, None),
            event(700, 950, Some("D:min")),
        ];
        assert_eq!(h.lookup(id, &path, &digest, 200, 950).unwrap(), expected);
        assert!(h.lookup(id, &path, &digest, 1000, 1100).unwrap().is_empty());
        worker.close(None).unwrap();
        let (store, _, _) = Store::open(db).unwrap();
        let sub = Subject { id, path, digest };
        assert_eq!(lookup(&store.connection, &sub, 200, 950).unwrap(), expected);
        let other = Subject {
            id: other_id,
            path: other,
            digest,
        };
        assert_eq!(
            lookup(&store.connection, &other, 0, 2000).unwrap(),
            [event(0, 900, Some("G:min"))]
        );
    }

    #[test]
    fn d01_confirmations_rollback_a_failed_split_and_limit_per_track() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut lib, _) = Store::open(dir.path().join("library.sqlite3")).unwrap();
        let path = fixture();
        lib.insert([path.clone()]);
        let id = lib.track_id_for_path(&path).unwrap();
        store.save_delta(&lib.changes()).unwrap();
        let s = Subject {
            id,
            path,
            digest: [8; 32],
        };
        let c = &mut store.connection;
        let original = event(0, 900, Some("A:min"));
        correct(c, &s, &original).unwrap();
        c.execute_batch(
            "CREATE TEMP TRIGGER fail_chord BEFORE INSERT ON track_chord_confirmations
            WHEN NEW.start_ms=100 BEGIN SELECT RAISE(ABORT,'injected split failure');END;",
        )
        .unwrap();
        assert!(correct(c, &s, &event(100, 200, Some("D:maj"))).is_err());
        assert_eq!(
            lookup(c, &s, 0, 1000).unwrap(),
            std::slice::from_ref(&original)
        );
        c.execute_batch("DROP TRIGGER fail_chord").unwrap();
        c.execute(
            "WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<?1)
            INSERT INTO track_chord_confirmations SELECT ?2,zeroblob(32),i*2,i*2+1,NULL FROM seq",
            params![MAX_CONFIRMED - 1, id],
        )
        .unwrap();
        assert!(correct(c, &s, &event(100, 200, None)).is_err());
        assert_eq!(lookup(c, &s, 0, 1000).unwrap(), [original]);
    }
}
