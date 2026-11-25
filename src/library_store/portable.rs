use super::{Command, Store, Worker};
use crate::portable_set::ImportedSet;
use rusqlite::{Connection, OptionalExtension};
use std::{path::PathBuf, sync::mpsc::*};

pub(super) fn schema(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS imported_sets(name TEXT PRIMARY KEY,project TEXT NOT NULL CHECK(length(CAST(project AS BLOB))<=8388608)) STRICT;").map_err(|e|e.to_string())
}
#[derive(Clone)]
pub(crate) struct Sets(SyncSender<Command>);
impl Worker {
    pub(crate) fn portable_sets(&self) -> Sets {
        Sets(self.tx.clone())
    }
}
pub(super) enum Request {
    Files(i32, Sender<Result<Vec<(i32, PathBuf)>, String>>),
    Names(Sender<Result<Vec<String>, String>>),
    Read(String, Sender<Result<ImportedSet, String>>),
    Save(String, String, Sender<Result<(), String>>),
}
impl Sets {
    fn call<T>(&self, req: impl FnOnce(Sender<Result<T, String>>) -> Request) -> Result<T, String> {
        let (tx, rx) = channel();
        self.0
            .try_send(Command::Portable(req(tx)))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
    pub(crate) fn files(&self, after: i32) -> Result<Vec<(i32, PathBuf)>, String> {
        self.call(|tx| Request::Files(after, tx))
    }
    pub(crate) fn names(&self) -> Result<Vec<String>, String> {
        self.call(Request::Names)
    }
    pub(crate) fn read(&self, name: String) -> Result<ImportedSet, String> {
        self.call(|tx| Request::Read(name, tx))
    }
    pub(crate) fn save(&self, name: String, set: &ImportedSet) -> Result<(), String> {
        if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err("Enter a set name (1–256 bytes).".into());
        }
        let json = serde_json::to_string(set).map_err(|e| e.to_string())?;
        if json.len() > 8 * 1024 * 1024 {
            return Err("Resolved set exceeds 8 MiB.".into());
        }
        self.call(|tx| Request::Save(name, json, tx))
    }
}
fn strings(db: &Connection, sql: &str) -> Result<Vec<String>, String> {
    let mut st = db.prepare(sql).map_err(|e| e.to_string())?;
    let row = st.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?;
    row.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}
impl Request {
    pub(super) fn run(self, store: &mut Store) {
        let db = &store.connection;
        match self {
            Self::Files(after, tx) => {
                let page = || -> rusqlite::Result<Vec<(i32, PathBuf)>> {
                    let mut stmt =
                        db.prepare("SELECT id,path FROM tracks WHERE id>? ORDER BY id LIMIT 256")?;
                    let rows = stmt.query_map([after], |row| {
                        Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
                    })?;
                    rows.collect()
                };
                let _ = tx.send(page().map_err(|e| e.to_string()));
            }
            Self::Names(tx) => {
                let _ = tx.send(strings(
                    db,
                    "SELECT name FROM imported_sets ORDER BY name COLLATE NOCASE",
                ));
            }
            Self::Read(name, tx) => {
                let result = db
                    .query_row(
                        "SELECT project FROM imported_sets WHERE name=?",
                        [name],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())
                    .and_then(|v| v.ok_or_else(|| "Choose an existing catalog set.".to_string()))
                    .and_then(|s| {
                        serde_json::from_str::<ImportedSet>(&s).map_err(|e| e.to_string())
                    })
                    .and_then(|s| {
                        s.manifest.validate()?;
                        if s.paths.len() != s.manifest.entries.len() {
                            return Err("Invalid stored track mappings.".into());
                        }
                        Ok(s)
                    });
                let _ = tx.send(result);
            }
            Self::Save(name, json, tx) => {
                let result=db.execute("INSERT INTO imported_sets(name,project) VALUES(?1,?2) ON CONFLICT(name) DO NOTHING",rusqlite::params![name,json])
                    .map_err(|e|e.to_string()).and_then(|n|if n==1 {Ok(())}else{Err("This set name already exists. Choose a new name to keep both versions.".into())});
                if result.is_ok() {
                    store.backup_pending = true;
                }
                let _ = tx.send(result);
            }
        }
    }
}
