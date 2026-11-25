//! Personal, position-specific wordplay annotations. Search uses the normalized word index.
use super::{Command, Store, Worker};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

pub(super) fn schema(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS hook_marks(
        id INTEGER PRIMARY KEY, track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        at_ns INTEGER NOT NULL CHECK(at_ns BETWEEN 0 AND 604800000000000),
        term TEXT NOT NULL CHECK(length(term) BETWEEN 1 AND 80), term_key TEXT NOT NULL CHECK(length(term_key) BETWEEN 1 AND 240),
        UNIQUE(term_key,track_id,at_ns)) STRICT;").map_err(|e|e.to_string())
}
#[derive(Clone)]
pub(crate) struct Mark {
    pub id: i64,
    pub track: i32,
    pub path: PathBuf,
    pub title: String,
    pub at: Duration,
    pub term: String,
}
pub(crate) enum Op {
    Search {
        query: String,
        after: Option<Mark>,
    },
    Save {
        original: Option<Mark>,
        track: i32,
        path: PathBuf,
        at: Duration,
        term: String,
    },
    Delete(Mark),
}
pub(crate) enum Outcome {
    Page(Vec<Mark>, bool),
    Changed,
}
pub(super) struct Request(Op, Sender<Result<Outcome, String>>);
impl Worker {
    pub(crate) fn hook_request(&self, op: Op) -> Result<Receiver<Result<Outcome, String>>, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Command::Hooks(Request(op, tx)))
            .map_err(|e| e.to_string())?;
        Ok(rx)
    }
}
fn words(raw: &str) -> Result<(String, String), String> {
    let word = raw.trim();
    if word.is_empty() || word.chars().count() > 80 || word.chars().any(char::is_control) {
        return Err("Enter a word or phrase of 1 to 80 characters.".into());
    }
    Ok((word.into(), word.to_lowercase()))
}
fn ns(at: Duration) -> Result<i64, String> {
    i64::try_from(at.as_nanos())
        .ok()
        .filter(|&v| v <= 604800000000000)
        .ok_or_else(|| "Time is outside the supported range.".into())
}
impl Request {
    pub(super) fn run(self, store: &mut Store) {
        let Self(op, reply) = self;
        let result = run(&mut store.connection, op);
        if matches!(result, Ok(Outcome::Changed)) {
            store.backup_pending = true;
        }
        let _ = reply.send(result);
    }
}
fn run(db: &mut Connection, op: Op) -> Result<Outcome, String> {
    let err = |e: rusqlite::Error| e.to_string();
    if let Op::Search { query, after } = &op {
        let key = if query.trim().is_empty() {
            String::new()
        } else {
            words(query)?.1
        };
        let cursor = after
            .as_ref()
            .map(|m| (m.term.to_lowercase(), m.track, ns(m.at)))
            .unwrap_or_else(|| (String::new(), 0, Ok(0)));
        let filter = if key.is_empty() {
            ""
        } else {
            "AND h.term_key=?4"
        };
        let sql=format!("SELECT h.id,h.track_id,t.path,COALESCE(m.title,''),h.at_ns,h.term FROM hook_marks h JOIN tracks t ON t.id=h.track_id LEFT JOIN track_metadata m ON m.track_id=t.id WHERE (h.term_key,h.track_id,h.at_ns)>(?1,?2,?3) {filter} ORDER BY h.term_key,h.track_id,h.at_ns LIMIT 51");
        let mut stmt = db.prepare(&sql).map_err(err)?;
        let at = cursor.2?;
        let mut rows = if key.is_empty() {
            stmt.query(params![cursor.0, cursor.1, at])
        } else {
            stmt.query(params![cursor.0, cursor.1, at, key])
        }
        .map_err(err)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(err)? {
            let path = PathBuf::from(row.get::<_, String>(2).map_err(err)?);
            let name: String = row.get(3).map_err(err)?;
            let at = u64::try_from(row.get::<_, i64>(4).map_err(err)?)
                .map_err(|_| "Invalid saved marker time.")?;
            result.push(Mark {
                id: row.get(0).map_err(err)?,
                track: row.get(1).map_err(err)?,
                title: if name.is_empty() {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    name
                },
                path,
                at: Duration::from_nanos(at),
                term: row.get(5).map_err(err)?,
            });
        }
        let more = result.len() > 50;
        result.truncate(50);
        return Ok(Outcome::Page(result, more));
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let original = match &op {
        Op::Save { original, .. } => original.as_ref(),
        Op::Delete(old) => Some(old),
        _ => None,
    };
    if let Some(m) = original {
        let key = crate::library::identity(&m.path);
        let affected=tx.execute("DELETE FROM hook_marks WHERE id=?1 AND track_id=?2 AND at_ns=?3 AND term=?4 AND EXISTS(SELECT 1 FROM tracks WHERE id=?2 AND path_key=?5)",
            params![m.id,m.track,ns(m.at)?,m.term,key.to_str().ok_or("Unsupported track path.")?]).map_err(err)?;
        if affected != 1 {
            return Err("Marker or source changed. Search again.".into());
        }
    }
    if let Op::Save {
        track,
        path,
        at,
        term,
        ..
    } = op
    {
        let (word, key) = words(&term)?;
        let at = ns(at)?;
        let identity = crate::library::identity(&path);
        let duration=tx.query_row("SELECT m.duration_ms FROM tracks t LEFT JOIN track_metadata m ON m.track_id=t.id WHERE t.id=?1 AND t.path_key=?2",params![track,identity.to_str().ok_or("Unsupported path.")?],|r|r.get::<_,Option<i64>>(0)).optional().map_err(err)?.ok_or("The track is no longer stored at this path.")?;
        if duration.is_some_and(|ms| at >= ms.saturating_mul(1_000_000)) {
            return Err("The mark must lie before the end of the track.".into());
        }
        tx.execute(
            "INSERT INTO hook_marks(track_id,at_ns,term,term_key) VALUES (?1,?2,?3,?4)",
            params![track, at, word, key],
        )
        .map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(Outcome::Changed)
}

#[cfg(test)]
mod qualification {
    use super::*;
    fn send(store: &mut Store, op: Op) -> Result<Outcome, String> {
        let (tx, rx) = mpsc::channel();
        Request(op, tx).run(store);
        rx.recv().unwrap()
    }
    fn search() -> Op {
        Op::Search {
            query: "HOOK".into(),
            after: None,
        }
    }
    #[test]
    fn r01_hook_edits_schedule_backup_and_preserve_both_track_timestamps_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        let (mut store, mut library, _) = Store::open(db.clone()).unwrap();
        library.insert([a.clone(), b.clone()]);
        store.save(&library.catalog()).unwrap();
        store.checkpoint(true).unwrap();
        for (path, seconds) in [(a, 12), (b, 47)] {
            let id = library.track_id_for_path(&path).unwrap();
            assert!(!store.backup_pending);
            send(&mut store, search()).unwrap();
            assert!(!store.backup_pending);
            send(
                &mut store,
                Op::Save {
                    original: None,
                    track: id,
                    path,
                    at: Duration::from_secs(seconds),
                    term: "Hook".into(),
                },
            )
            .unwrap();
            assert!(store.backup_pending);
            store.checkpoint(true).unwrap();
        }
        let backup = Connection::open(super::super::side_path(&db, ".bak")).unwrap();
        assert_eq!(
            backup
                .query_row("SELECT count(*) FROM hook_marks", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(backup);
        drop(store);
        let (mut store, _, _) = Store::open(db).unwrap();
        let Outcome::Page(mut marks, false) = send(&mut store, search()).unwrap() else {
            panic!("expected two results")
        };
        assert_eq!(
            marks.iter().map(|m| m.at.as_secs()).collect::<Vec<_>>(),
            [12, 47]
        );
        let removed = marks.remove(0);
        send(&mut store, Op::Delete(removed.clone())).unwrap();
        assert!(store.backup_pending);
        store.checkpoint(true).unwrap();
        assert!(send(&mut store, Op::Delete(removed)).is_err());
        assert!(!store.backup_pending);
        let Outcome::Page(marks, false) = send(&mut store, search()).unwrap() else {
            panic!("expected one result")
        };
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].at.as_secs(), 47);
    }
}
