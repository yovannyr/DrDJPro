use super::{read_grid_row, Command, Store, Worker};
use crate::beatgrid::{BeatGrid, TempoComparison};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

enum GridEdit {
    Tempo,
    Lock,
}

pub(super) struct PrepareGridWrite {
    id: i32,
    path: PathBuf,
    expected: Option<BeatGrid>,
    edit: GridEdit,
    grid: BeatGrid,
    pub(super) reply: mpsc::Sender<Result<(), String>>,
}

impl Worker {
    /// Persist just the selected track's constant grid and acknowledge its transaction.
    pub fn commit_prepare_grid(
        &self,
        id: i32,
        path: &Path,
        expected: Option<BeatGrid>,
        grid: BeatGrid,
    ) -> Result<(), String> {
        self.write_prepare_grid(id, path, expected, GridEdit::Tempo, grid)
    }

    pub fn set_prepare_grid_lock(
        &self,
        id: i32,
        path: &Path,
        expected: BeatGrid,
        locked: bool,
    ) -> Result<(), String> {
        self.write_prepare_grid(
            id,
            path,
            Some(expected),
            GridEdit::Lock,
            expected.with_locked(locked),
        )
    }

    fn write_prepare_grid(
        &self,
        id: i32,
        path: &Path,
        expected: Option<BeatGrid>,
        edit: GridEdit,
        grid: BeatGrid,
    ) -> Result<(), String> {
        let (reply, acknowledgement) = mpsc::channel();
        self.tx
            .try_send(Command::PrepareGrid(Box::new(PrepareGridWrite {
                id,
                path: path.to_owned(),
                expected,
                edit,
                grid,
                reply,
            })))
            .map_err(|e| format!("Grid save unavailable: {e}"))?;
        // An arbitrary timeout could report failure while a queued transaction commits later.
        acknowledgement.recv().map_err(|e| e.to_string())?
    }
}

impl Store {
    pub(super) fn apply_prepare_grid(&mut self, request: &PrepareGridWrite) -> Result<(), String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let key = crate::library::identity(&request.path);
        let path_key: Option<String> = tx
            .query_row(
                "SELECT path_key FROM tracks WHERE id=?1",
                [request.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if path_key.is_none() || path_key.as_deref() != key.to_str() {
            return Err(
                "Track changed or has not finished saving; reopen the tempo comparison".into(),
            );
        }
        let current = {
            let mut statement = tx.prepare("SELECT track_id,bpm,anchor_ns,beats_per_bar,base_meter_denominator,meter_changes,tempo_changes,warp_markers,locked FROM track_grids WHERE track_id=?1")
                .map_err(|e| e.to_string())?;
            let mut rows = statement.query([request.id]).map_err(|e| e.to_string())?;
            rows.next()
                .map_err(|e| e.to_string())?
                .map(read_grid_row)
                .transpose()
                .map_err(|e| format!("Cannot read saved grid: {e:?}"))?
        };
        if current != request.expected {
            return Err("Saved grid changed; reopen the tempo comparison".into());
        }
        let grid = &request.grid;
        match request.edit {
            GridEdit::Lock => {
                current.ok_or("Save a beatgrid before locking it")?;
                tx.execute(
                    "UPDATE track_grids SET locked=?1 WHERE track_id=?2",
                    params![grid.is_locked(), request.id],
                )
                .map_err(|e| e.to_string())?;
            }
            GridEdit::Tempo => {
                for source in [current.as_ref(), Some(grid)].into_iter().flatten() {
                    if let Some(reason) = TempoComparison::protection(source) {
                        return Err(reason.into());
                    }
                }
                if current.is_some_and(|saved| {
                    saved.beats_per_bar() != grid.beats_per_bar()
                        || saved.meter_denominator() != grid.meter_denominator()
                }) {
                    return Err("Tempo comparison cannot change the meter".into());
                }
                tx.execute("INSERT INTO track_grids(track_id,bpm,anchor_ns,beats_per_bar,base_meter_denominator) VALUES(?1,?2,?3,?4,?5)
                    ON CONFLICT(track_id) DO UPDATE SET bpm=excluded.bpm,anchor_ns=excluded.anchor_ns",
                    params![request.id, grid.bpm(), grid.anchor().as_nanos() as i64, grid.beats_per_bar(), grid.meter_denominator()])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.backup_pending = true;
        Ok(())
    }
}
