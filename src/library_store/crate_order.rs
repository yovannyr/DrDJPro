//! Saved performance order for ordinary crates. Membership remains authoritative.
use super::{Command, Store, Worker};
use crate::crate_order::MAX_ENTRIES;
use rusqlite::{params, Connection, TransactionBehavior};
use std::sync::mpsc;

pub(super) fn schema(db: &Connection) -> Result<(), String> {
    // A full catalog save replaces the crate rows inside one transaction. The order survives
    // that rebuild; the snapshot writer prunes rows whose membership actually disappeared.
    db.execute_batch("CREATE TABLE IF NOT EXISTS crate_set_order(
        crate_id INTEGER NOT NULL, track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        position INTEGER NOT NULL CHECK(position>=0), PRIMARY KEY(crate_id,track_id),
        UNIQUE(crate_id,position)) STRICT;").map_err(|e| e.to_string())
}

pub(super) enum Request {
    Read(i32, mpsc::Sender<Result<Vec<i32>, String>>),
    Saved(i32, mpsc::Sender<Result<Option<Vec<i32>>, String>>),
    Replace {
        id: i32,
        expected: Vec<i32>,
        replacement: Vec<i32>,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

#[derive(Clone)]
pub(crate) struct CrateOrders {
    tx: mpsc::SyncSender<Command>,
}

impl Worker {
    pub(crate) fn crate_orders(&self) -> CrateOrders {
        CrateOrders {
            tx: self.tx.clone(),
        }
    }
}

impl CrateOrders {
    pub(crate) fn saved_crate_order(&self, id: i32) -> Result<Option<Vec<i32>>, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Command::Order(Request::Saved(id, tx)))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
    pub(crate) fn crate_order(&self, id: i32) -> Result<Vec<i32>, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Command::Order(Request::Read(id, tx)))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    pub(crate) fn begin_replace_crate_order(
        &self,
        id: i32,
        expected: &[i32],
        replacement: &[i32],
    ) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        if expected.len() != replacement.len() || expected.len() > MAX_ENTRIES {
            return Err("Set list size changed or exceeds the preparation limit.".into());
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Command::Order(Request::Replace {
                id,
                expected: expected.to_vec(),
                replacement: replacement.to_vec(),
                reply: tx,
            }))
            .map_err(|e| e.to_string())?;
        Ok(rx)
    }
}

fn read(db: &Connection, id: i32) -> Result<Vec<i32>, String> {
    let valid: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM crates WHERE id=? AND smart_query IS NULL)",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !valid {
        return Err("Choose an existing ordinary crate.".into());
    }
    let mut statement = db
        .prepare(
            "SELECT m.track_id FROM crate_tracks m
        LEFT JOIN crate_set_order o ON m.crate_id=o.crate_id AND m.track_id=o.track_id
        WHERE m.crate_id=?1 ORDER BY o.position IS NULL,o.position,m.track_id LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![id, (MAX_ENTRIES + 1) as i64], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let ids = rows
        .collect::<rusqlite::Result<Vec<i32>>>()
        .map_err(|e| e.to_string())?;
    if ids.len() > MAX_ENTRIES {
        return Err(format!(
            "Choose a crate with at most {MAX_ENTRIES} entries."
        ));
    }
    Ok(ids)
}

fn replace(store: &mut Store, id: i32, expected: &[i32], proposed: &[i32]) -> Result<(), String> {
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if read(&tx, id)? != expected {
        return Err("The saved set or its membership changed. Generate a fresh preview.".into());
    }
    let mut before = expected.to_vec();
    let mut after = proposed.to_vec();
    before.sort_unstable();
    after.sort_unstable();
    if before != after {
        return Err("The preview does not contain the same entries.".into());
    }
    tx.execute("DELETE FROM crate_set_order WHERE crate_id=?", [id])
        .map_err(|e| e.to_string())?;
    {
        let mut put = tx
            .prepare("INSERT INTO crate_set_order(crate_id,track_id,position) VALUES(?1,?2,?3)")
            .map_err(|e| e.to_string())?;
        for (index, track) in proposed.iter().enumerate() {
            put.execute(params![id, track, index as i64])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    store.backup_pending = true;
    Ok(())
}

impl Request {
    pub(super) fn run(self, store: &mut Store) {
        match self {
            Self::Saved(id, reply) => {
                let exists = store.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM crate_set_order WHERE crate_id=?)",
                    [id],
                    |r| r.get::<_, bool>(0),
                );
                let result = exists.map_err(|e| e.to_string()).and_then(|saved| {
                    if saved {
                        read(&store.connection, id).map(Some)
                    } else {
                        Ok(None)
                    }
                });
                let _ = reply.send(result);
            }
            Self::Read(id, reply) => {
                let _ = reply.send(read(&store.connection, id));
            }
            Self::Replace {
                id,
                expected,
                replacement,
                reply,
            } => {
                let _ = reply.send(replace(store, id, &expected, &replacement));
            }
        }
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    #[test]
    fn r06_committed_crate_order_survives_reopen_and_rejects_stale_membership() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        let paths: Vec<_> = (1..=12)
            .map(|i| dir.path().join(format!("{i}.wav")))
            .collect();
        let (mut store, mut lib, _) = Store::open(db.clone()).unwrap();
        lib.insert(paths.clone());
        let id = lib.create_crate("Set").unwrap();
        lib.add_paths_to_crate(id, &paths).unwrap();
        store.save(&lib.catalog()).unwrap();
        let expected = read(&store.connection, id).unwrap();
        let mut next = expected.clone();
        next.swap(3, 4);
        replace(&mut store, id, &expected, &next).unwrap();
        drop(store);
        let (mut store, _, _) = Store::open(db).unwrap();
        assert_eq!(read(&store.connection, id).unwrap(), next);
        store
            .connection
            .execute(
                "DELETE FROM crate_tracks WHERE crate_id=?1 AND track_id=?2",
                params![id, next[2]],
            )
            .unwrap();
        assert!(replace(&mut store, id, &next, &expected).is_err());
        let after = read(&store.connection, id).unwrap();
        assert_eq!(
            after,
            next.iter()
                .copied()
                .filter(|v| *v != next[2])
                .collect::<Vec<_>>()
        );
    }
}
