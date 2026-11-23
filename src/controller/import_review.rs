//! An import batch is owned here until the DJ confirms its current evidence.
use super::*;
use crate::cancellation::Cancellation;
use crate::library::import_review::{self as detection, ImportFinding, MatchKind};
use crate::library::{identity, ImportBatch, ImportCatalog, ImportEvidence};
use crate::track_metadata::TrackMetadata;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewAction {
    Apply,
    Rescan,
}

pub(super) struct ImportSession {
    batch: ImportBatch,
    // Only new or changed sources need A2 analysis after an explicit Keep.
    deferred: HashSet<PathBuf>,
    rows: Rc<ReviewRows>,
    crate_id: i32,
    sync: bool,
}

struct ReviewRows {
    inputs: Arc<Vec<PathBuf>>,
    issues: HashMap<usize, ImportFinding>,
    eligible: Vec<bool>,
    selected: RefCell<Vec<bool>>,
    notify: slint::ModelNotify,
}

fn scan(
    mut batch: ImportBatch,
    catalog: ImportCatalog,
    cancel: &Cancellation,
) -> Result<ImportBatch, String> {
    let mut unavailable = HashSet::new();
    batch
        .review
        .unavailable
        .retain(|(path, _)| unavailable.insert(identity(path)));
    let mut seen = HashSet::new();
    let mut tags = std::mem::take(&mut batch.review.stamps).into_iter();
    let mut before = Vec::new();
    let input_count = batch.paths.len();
    batch.paths.retain(|path| {
        let tag_stamp = tags.next().flatten();
        let key = identity(path);
        if !unavailable.contains(&key) && seen.insert(key) {
            before.push(tag_stamp);
            true
        } else {
            false
        }
    });
    batch.rejected += input_count - batch.paths.len();
    seen.clear();
    batch.metadata.retain(|(path, _)| {
        let key = identity(path);
        !unavailable.contains(&key) && seen.insert(key)
    });
    let findings = if batch.paths.is_empty() {
        Vec::new()
    } else {
        detection::inspect(&batch, catalog.references(), cancel)?
    };
    let mut by_path: HashMap<_, _> = findings
        .into_iter()
        .map(|f| (identity(&f.path), f))
        .collect();
    for (path, original) in batch.paths.iter().zip(before) {
        cancel.check()?;
        let current = detection::Stamp::at(path).ok();
        if current.is_none() || original != current {
            batch.review.stamps.push(None);
            batch.review.findings.push(ImportFinding::unchecked(
                path.clone(),
                "Source disappeared or changed during inspection".into(),
            ));
        } else {
            batch.review.stamps.push(current);
            if let Some(f) = by_path.remove(&identity(path)) {
                batch.review.findings.push(f);
            }
        }
    }
    batch.review.findings.extend(
        batch
            .review
            .unavailable
            .iter()
            .map(|(path, reason)| ImportFinding::unchecked(path.clone(), reason.clone())),
    );
    batch.review.catalog_revision = Some(catalog.revision);
    cancel.check()?;
    Ok(batch)
}

fn recheck_batch(
    inputs: &[PathBuf],
    catalog: ImportCatalog,
    late: Option<(ImportEvidence, HashSet<PathBuf>)>,
    cancel: &Cancellation,
    mut analyze: impl FnMut(&Path, &mut TrackMetadata, &Cancellation) -> Result<(), String>,
) -> Result<ImportBatch, String> {
    let headers = || {
        scan(
            crate::library::inspect_review_headers(inputs.to_vec(), cancel)?,
            catalog.clone(),
            cancel,
        )
    };
    let mut fresh = headers()?;
    let Some((expected, paths)) = late else {
        return Ok(fresh);
    };
    if fresh.review != expected
        || Some(fresh.paths.as_slice()) != inputs.get(..expected.stamps.len())
    {
        return Ok(fresh);
    }
    for (path, meta) in &mut fresh.metadata {
        if paths.contains(path) {
            analyze(path, meta, cancel)?;
        }
    }
    // Decoding can take time. Recheck both the incoming files and their references afterwards.
    let checked = headers()?;
    if fresh.paths != checked.paths || fresh.review != checked.review {
        return Ok(checked);
    }
    Ok(fresh)
}

/// Moves cached analysis, including its waveform buffer, only while the native source is stable.
fn carry_forward(previous: &mut ImportSession, fresh: &mut ImportBatch) -> HashSet<PathBuf> {
    let indices: HashMap<_, _> = previous
        .batch
        .paths
        .iter()
        .enumerate()
        .map(|(i, path)| (path.as_path(), i))
        .collect();
    let mut deferred = HashSet::new();
    for (i, (path, metadata)) in fresh.metadata.iter_mut().enumerate() {
        let prior = indices.get(path.as_path()).copied().filter(|&j| {
            let stamp = fresh.review.stamps.get(i).and_then(Option::as_ref);
            stamp.is_some() && stamp == previous.batch.review.stamps.get(j).and_then(Option::as_ref)
        });
        if let Some(j) = prior {
            if metadata.analysis.is_none() {
                metadata.analysis = previous.batch.metadata[j].1.analysis.take();
            }
            if !previous.deferred.contains(path) {
                continue;
            }
        }
        deferred.insert(path.clone());
    }
    deferred
}

impl ReviewRows {
    fn new(batch: &ImportBatch) -> Self {
        let inputs: Vec<_> = batch
            .paths
            .iter()
            .chain(batch.review.unavailable.iter().map(|(path, _)| path))
            .cloned()
            .collect();
        let indices: HashMap<_, _> = inputs
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_path(), i))
            .collect();
        let issues: HashMap<_, _> = batch
            .review
            .findings
            .iter()
            .filter_map(|f| Some((*indices.get(f.path.as_path())?, f.clone())))
            .collect();
        let eligible: Vec<_> = (0..inputs.len())
            .map(|i| batch.review.stamps.get(i).is_some_and(Option::is_some))
            .collect();
        let selected = eligible
            .iter()
            .enumerate()
            .map(|(i, enabled)| *enabled && !issues.contains_key(&i))
            .collect();
        Self {
            inputs: Arc::new(inputs),
            issues,
            eligible,
            selected: RefCell::new(selected),
            notify: Default::default(),
        }
    }
    fn toggle(&self, index: i32) {
        let Ok(i) = usize::try_from(index) else {
            return;
        };
        if self.eligible.get(i) == Some(&true) {
            let mut selected = self.selected.borrow_mut();
            selected[i] = !selected[i];
            drop(selected);
            self.notify.row_changed(i);
        }
    }
    fn keep_selected(&self, batch: &mut ImportBatch) {
        let selected = self.selected.borrow();
        let chosen: HashSet<_> = self
            .inputs
            .iter()
            .zip(selected.iter())
            .filter(|(_, keep)| **keep)
            .map(|(path, _)| path.as_path())
            .collect();
        batch.paths.retain(|path| chosen.contains(path.as_path()));
        batch
            .metadata
            .retain(|(path, _)| chosen.contains(path.as_path()));
    }
}

impl Model for ReviewRows {
    type Data = crate::ImportReviewRow;
    fn row_count(&self) -> usize {
        self.inputs.len()
    }
    fn row_data(&self, row: usize) -> Option<Self::Data> {
        let path = self.inputs.get(row)?;
        let matches = self.issues.get(&row).map_or_else(
            || String::from("No duplicate or variant found"),
            |f| {
                let label = match f.kind {
                    MatchKind::Exact => "Exact copy",
                    MatchKind::Variant => "Possible edit",
                    MatchKind::Unchecked => "Not checked",
                };
                if let Some(other) = &f.reference {
                    format!("{label}: {}\n{}", other.display(), f.detail)
                } else {
                    format!("{label}: {}", f.detail)
                }
            },
        );
        Some(crate::ImportReviewRow {
            index: i32::try_from(row).ok()?,
            path: path.to_string_lossy().as_ref().into(),
            matches: matches.into(),
            selected: self.selected.borrow()[row],
            eligible: self.eligible[row],
        })
    }
    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl AudioController {
    pub(super) fn wire_import_review(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_apply_import_review(move || {
            if let Some(c) = weak.upgrade() {
                c.check_import_review(ReviewAction::Apply);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_rescan_import_review(move || {
            if let Some(c) = weak.upgrade() {
                c.check_import_review(ReviewAction::Rescan);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_cancel_import_review(move || {
            if let Some(c) = weak.upgrade() {
                if c.review_action.get().is_some() {
                    if let Some(cancel) = c.pending_cancel.borrow().as_ref() {
                        cancel.request();
                    }
                }
                c.close_import_review();
                if let Some(ui) = c.ui.upgrade() {
                    ui.set_status("Library import cancelled".into());
                }
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_toggle_import_review(move |index| {
            if let Some(c) = weak.upgrade() {
                if c.review_action.get().is_none() {
                    if let Some(session) = c.import_review.borrow().as_ref() {
                        session.rows.toggle(index);
                    }
                }
            }
        });
    }

    pub(super) fn start_reviewed_import(
        &self,
        work: impl FnOnce(&Cancellation) -> ImportResult + Send + 'static,
    ) {
        let source = self.library.borrow().import_catalog();
        self.start_import_work(move |cancel| {
            work(cancel).map(|result| result.and_then(|batch| scan(batch, source, cancel)))
        });
    }

    pub(super) fn receive_import_batch(&self, batch: ImportBatch, crate_id: i32, sync: bool) {
        let stale = self.stale_import_evidence(&batch);
        if stale || !batch.review.findings.is_empty() {
            self.show_import_review(
                batch,
                crate_id,
                sync,
                HashSet::new(),
                if stale {
                    "Library changed during inspection. Rescan before importing."
                } else {
                    "Check the source and matching paths. Keep or skip each file, then apply."
                },
            );
        } else {
            self.apply_import_batch(batch, crate_id, sync);
        }
    }

    fn stale_import_evidence(&self, batch: &ImportBatch) -> bool {
        batch
            .review
            .catalog_revision
            .is_some_and(|seen| seen != self.library.borrow().import_catalog().revision)
    }

    fn show_import_review(
        &self,
        batch: ImportBatch,
        crate_id: i32,
        sync: bool,
        deferred: HashSet<PathBuf>,
        notice: &str,
    ) {
        let Some(ui) = self.ui.upgrade() else { return };
        let rows = Rc::new(ReviewRows::new(&batch));
        let empty = rows.inputs.is_empty();
        if empty {
            self.close_import_review();
            ui.set_status("No import candidates remain".into());
            return;
        }
        *self.import_review.borrow_mut() = Some(ImportSession {
            batch,
            deferred,
            rows: Rc::clone(&rows),
            crate_id,
            sync,
        });
        ui.set_import_review_rows(ModelRc::new(rows));
        ui.set_import_review_checking(false);
        ui.set_import_review_status(notice.into());
        ui.set_modern_library_open(true);
        ui.set_import_review_open(true);
        ui.set_status("Import review awaiting selection".into());
    }

    pub(super) fn close_import_review(&self) {
        self.import_review.borrow_mut().take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_import_review_open(false);
            ui.set_import_review_rows(Default::default());
            ui.set_import_review_checking(false);
        }
    }

    fn check_import_review(&self, action: ReviewAction) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.review_action.get().is_some() || self.pending_import.borrow().is_some() {
            return;
        }
        let Some((inputs, late)) = self.import_review.borrow().as_ref().map(|session| {
            let late = if action == ReviewAction::Apply && !session.deferred.is_empty() {
                let selected: HashSet<_> = session
                    .rows
                    .inputs
                    .iter()
                    .zip(session.rows.selected.borrow().iter())
                    .filter(|(p, keep)| **keep && session.deferred.contains(p.as_path()))
                    .map(|(p, _)| p.clone())
                    .collect();
                (!selected.is_empty()).then(|| (session.batch.review.clone(), selected))
            } else {
                None
            };
            (Arc::clone(&session.rows.inputs), late)
        }) else {
            return;
        };
        let catalog = self.library.borrow().import_catalog();
        self.review_action.set(Some(action));
        self.start_import_work(move |cancel| {
            Some(recheck_batch(
                &inputs,
                catalog,
                late,
                cancel,
                crate::library::analyze_import_metadata,
            ))
        });
        if self.pending_import.borrow().is_none() {
            self.review_action.set(None);
            ui.set_import_review_status(
                "Cannot start the check while the library is busy. Retry when ready.".into(),
            );
        } else {
            ui.set_import_review_checking(true);
            ui.set_import_review_status("Rechecking source files and matches…".into());
        }
    }

    pub(super) fn finish_import_review(
        &self,
        action: ReviewAction,
        result: Result<ImportResult, &str>,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((target, sync)) = self
            .import_review
            .borrow()
            .as_ref()
            .map(|s| (s.crate_id, s.sync))
        else {
            return;
        };
        ui.set_import_review_checking(false);
        let mut checked = match result {
            Ok(Some(Ok(batch))) => batch,
            Ok(Some(Err(e))) => {
                ui.set_import_review_status(e.into());
                return;
            }
            Err(e) => {
                ui.set_import_review_status(e.into());
                return;
            }
            Ok(None) => {
                ui.set_import_review_status(
                    "Check cancelled. Your selection is still open.".into(),
                );
                return;
            }
        };
        if let Some(previous) = self.import_review.borrow().as_ref() {
            // Rescans receive only distinct audio candidates; keep the original skipped count.
            checked.rejected += previous
                .batch
                .rejected
                .saturating_sub(previous.batch.review.unavailable.len());
        }
        let unchanged =
            self.import_review.borrow().as_ref().is_some_and(|s| {
                s.batch.paths == checked.paths && s.batch.review == checked.review
            }) && !self.stale_import_evidence(&checked);
        if action == ReviewAction::Rescan && unchanged {
            ui.set_import_review_status(
                "Rescan complete. Your previous choices are still selected.".into(),
            );
        } else if action == ReviewAction::Apply && unchanged {
            if let Some(s) = self.import_review.borrow_mut().as_mut() {
                carry_forward(s, &mut checked);
                let candidates = checked.paths.len();
                s.rows.keep_selected(&mut checked);
                checked.rejected += candidates - checked.paths.len();
            }
            self.close_import_review();
            if checked.paths.is_empty() {
                ui.set_status("No tracks selected for import".into());
                return;
            }
            self.apply_import_batch(checked, target, sync);
        } else {
            let deferred = self
                .import_review
                .borrow_mut()
                .as_mut()
                .map(|prior| carry_forward(prior, &mut checked))
                .unwrap_or_default();
            self.show_import_review(
                checked,
                target,
                sync,
                deferred,
                "Files or matches changed. Review the latest evidence before applying.",
            );
        }
    }

    fn apply_import_batch(&self, batch: ImportBatch, crate_id: i32, sync: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let mut library = self.library.borrow_mut();
        if crate_id != 0
            && !library
                .crates()
                .entries
                .iter()
                .any(|c| c.id == crate_id && c.smart_query.is_none())
        {
            ui.set_status(
                "Import target crate is unavailable. Choose a crate and import again.".into(),
            );
            return;
        }
        let total = batch.paths.len();
        let added = library.insert(batch.paths.iter().cloned());
        let assigned = library.add_paths_to_crate(crate_id, &batch.paths);
        for (path, data) in batch.metadata {
            library.set_metadata(&path, data);
        }
        drop(library);
        self.library_changed();
        if sync {
            self.sync_session_from_engine();
        }
        ui.set_status(
            match assigned {
                Ok(_) => format!(
                    "Added {added} tracks; {} skipped (duplicate, unsupported, missing or limit)",
                    total - added + batch.rejected
                ),
                Err(e) => format!("Added {added} library tracks; crate assignment failed: {e}"),
            }
            .into(),
        );
        self.publish_library();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use crate::test_support::wav_file;

    #[test]
    fn review_reuses_stable_analysis_and_only_decodes_after_confirming_new_evidence() {
        let first = wav_file(123, 4096);
        let second = wav_file(456, 2800);
        let both = [
            first.path().canonicalize().unwrap(),
            second.path().canonicalize().unwrap(),
        ];
        let cancel = Cancellation::default();
        let catalog = Library::default().import_catalog();
        let without_inference = |_: &Path, _: &mut TrackMetadata, _: &Cancellation| {
            panic!("Header/evidence check must not run full audio analysis");
        };
        let mut batch =
            recheck_batch(&both, catalog.clone(), None, &cancel, without_inference).unwrap();
        let old_analysis: crate::audio_analyzer::TrackAnalysisResult = serde_json::from_str(
            r#"{"file_path":"test","bpm":120,"gain":1,"musical_key":"","waveform_overview":[127],"is_corrupt":false}"#
        ).unwrap();
        let waveform = old_analysis.waveform_overview.as_ptr();
        batch.metadata[0].1.analysis = Some(old_analysis);
        let mut previous = ImportSession {
            rows: Rc::new(ReviewRows::new(&batch)),
            batch,
            deferred: HashSet::new(),
            sync: false,
            crate_id: 0,
        };

        // Replacing one source invalidates an explicit decision. The other source keeps its cached buffer.
        let replacement = wav_file(777, 3600);
        std::fs::copy(replacement.path(), second.path()).unwrap();
        let selected = HashSet::from([both[1].clone()]);
        let mut changed = recheck_batch(
            &both,
            catalog.clone(),
            Some((previous.batch.review.clone(), selected.clone())),
            &cancel,
            without_inference,
        )
        .unwrap();
        assert!(changed.review != previous.batch.review);
        let deferred = carry_forward(&mut previous, &mut changed);
        assert_eq!(deferred, selected);
        assert_eq!(
            changed.metadata[0]
                .1
                .analysis
                .as_ref()
                .unwrap()
                .waveform_overview
                .as_ptr(),
            waveform
        );
        assert!(changed.metadata[1].1.analysis.is_none());

        let calls = Cell::new(0);
        let verified = recheck_batch(
            &both,
            catalog.clone(),
            Some((changed.review, selected.clone())),
            &cancel,
            |path, _, _| {
                assert_eq!(path, both[1]);
                calls.set(calls.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 1);
        // A replacement during optional analysis must also invalidate the result.
        let expected = verified.review;
        let replaced = recheck_batch(
            &both,
            catalog,
            Some((expected.clone(), selected)),
            &cancel,
            |_, _, _| {
                std::fs::copy(replacement.path(), first.path()).unwrap();
                Ok(())
            },
        )
        .unwrap();
        assert!(replaced.review != expected);
    }
}
