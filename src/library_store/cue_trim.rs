use super::{
    prepare_grid::saved_prepare_grid, Command, Store, Worker, APPLICATION_ID, SCHEMA_VERSION,
};
use crate::hot_cues::CueTrimChange;
use rusqlite::{params, TransactionBehavior};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

pub(super) struct CueTrimWrite {
    id: i32,
    path: PathBuf,
    change: CueTrimChange,
    pub(super) reply: mpsc::Sender<Result<(), String>>,
}

impl Worker {
    /// A successful reply acknowledges the commit of this one existing cue position.
    pub(crate) fn commit_cue_trim(
        &self,
        id: i32,
        path: &Path,
        change: &CueTrimChange,
    ) -> Result<(), String> {
        change.validate()?;
        let (reply, acknowledgement) = mpsc::channel();
        self.tx
            .try_send(Command::CueTrim(Box::new(CueTrimWrite {
                id,
                path: path.to_owned(),
                change: change.clone(),
                reply,
            })))
            .map_err(|error| format!("Cue storage unavailable: {error}"))?;
        acknowledgement.recv().map_err(|error| error.to_string())?
    }
}

impl Store {
    pub(super) fn apply_cue_trim(&mut self, request: &CueTrimWrite) -> Result<(), String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let change = &request.change;
        let key = crate::library::identity(&request.path);
        let matches: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tracks WHERE id=?1 AND path_key=?2)
             AND (SELECT user_version FROM pragma_user_version)=?3
             AND (SELECT application_id FROM pragma_application_id)=?4",
                params![
                    request.id,
                    key.to_str().ok_or("Invalid track path")?,
                    SCHEMA_VERSION,
                    APPLICATION_ID
                ],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !matches {
            return Err("Track or catalog changed. Reopen the cue editor.".into());
        }
        if let Some(expected) = change.expected_grid {
            if saved_prepare_grid(&tx, request.id)? != expected {
                return Err("The saved beatgrid changed. Reopen the cue editor.".into());
            }
        }
        if let Some((slot, reference)) = &change.reference {
            let matches: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM track_hot_cues WHERE track_id=?1 AND slot=?2
                 AND position_ns=?3 AND name=?4 AND rgb=?5)",
                    params![
                        request.id,
                        *slot as i64,
                        reference.position().as_nanos() as i64,
                        reference.name(),
                        reference.rgb()
                    ],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !matches {
                return Err("The saved reference cue changed. Choose the reference again.".into());
            }
        }
        let original = &change.original;
        let updated = tx
            .execute(
                "UPDATE track_hot_cues SET position_ns=?1 WHERE track_id=?2 AND slot=?3
             AND position_ns=?4 AND name=?5 AND rgb=?6",
                params![
                    change.position.as_nanos() as i64,
                    request.id,
                    change.slot as i64,
                    original.position().as_nanos() as i64,
                    original.name(),
                    original.rgb()
                ],
            )
            .map_err(|e| e.to_string())?;
        if updated != 1 {
            return Err("The saved cue changed. Reopen the cue editor.".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.backup_pending = true;
        Ok(())
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    use crate::hot_cues::HotCue;
    use std::time::Duration;
    #[test]
    fn d05_exact_cue_commit_survives_restart_and_rejects_a_replaced_reference() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("library.sqlite3");
        let p = temp.path().join("one.wav");
        let (mut store, mut lib, _) = Store::open(db.clone()).unwrap();
        lib.insert([p.clone()]);
        let id = lib.track_id_for_path(&p).unwrap();
        let cue = |at, name: &str| {
            HotCue::new(Duration::from_millis(at), name.to_owned(), 0x112233).unwrap()
        };
        let trigger = cue(1030, "Cue");
        let reference = cue(1030, "Reference");
        lib.set_hot_cue(&p, 0, Some(trigger.clone())).unwrap();
        lib.set_hot_cue(&p, 1, Some(reference.clone())).unwrap();
        store.save(&lib.catalog()).unwrap();
        let change = CueTrimChange {
            slot: 0,
            original: trigger,
            reference: Some((1, reference.clone())),
            expected_grid: Some(None),
            position: Duration::from_millis(1155),
        };
        let req = CueTrimWrite {
            id,
            path: p.clone(),
            change,
            reply: mpsc::channel().0,
        };
        store.apply_cue_trim(&req).unwrap();
        drop(store);
        let (mut store, mut lib, _) = Store::open(db).unwrap();
        assert_eq!(
            lib.track_by_id(id).unwrap().hot_cues.slots()[0]
                .as_ref()
                .unwrap()
                .position(),
            Duration::from_millis(1155)
        );
        assert_eq!(
            lib.track_by_id(id).unwrap().hot_cues.slots()[1],
            Some(reference)
        );
        let original = lib.track_by_id(id).unwrap().hot_cues.slots()[0]
            .clone()
            .unwrap();
        lib.set_hot_cue(&p, 1, None).unwrap();
        store.save(&lib.catalog()).unwrap();
        let req = CueTrimWrite {
            change: CueTrimChange {
                original,
                position: Duration::ZERO,
                ..req.change
            },
            ..req
        };
        assert!(store.apply_cue_trim(&req).is_err());
    }
}
