//! Read the selected Prepare track's metadata on a bounded background worker.
use super::*;
use crate::{
    cancellation::Cancellation,
    track_metadata::diagnosis::{diagnose, CatalogValues, Diagnosis},
};

struct Source {
    id: i32,
    path: PathBuf,
    generation: u64,
    revision: u64,
}

struct Job {
    cancel: Cancellation,
    result: Receiver<Result<Diagnosis, String>>,
}

impl Job {
    fn start(path: PathBuf, catalog: CatalogValues) -> Result<Self, String> {
        let (tx, result) = mpsc::sync_channel(1);
        let cancel = Cancellation::default();
        let token = cancel.clone();
        std::thread::Builder::new()
            .name("audio-tag-reader".into())
            .spawn(move || {
                let _ = tx.send(diagnose(&path, &catalog, &token));
            })
            .map_err(|error| format!("Cannot start metadata diagnosis: {error}"))?;
        Ok(Self { cancel, result })
    }
}

#[derive(Default)]
pub(super) struct State {
    source: Option<Source>,
    pending: Option<CatalogValues>,
    // A cancelled read keeps the slot until it finishes, even after closing the panel.
    job: Option<Job>,
}

impl State {
    fn poll(&mut self) -> Option<Result<Diagnosis, String>> {
        if let Some(job) = &self.job {
            let result = match job.result.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err("Metadata diagnosis worker stopped".into()),
            };
            let cancelled = job.cancel.is_cancelled();
            self.job = None;
            if !cancelled {
                return Some(result);
            }
        }
        if let (Some(source), Some(catalog)) = (&self.source, self.pending.take()) {
            match Job::start(source.path.clone(), catalog) {
                Ok(job) => self.job = Some(job),
                Err(error) => return Some(Err(error)),
            }
        }
        None
    }
}

impl Source {
    fn is_current(&self, c: &AudioController, ui: &AppWindow) -> bool {
        let library = c.library.borrow();
        ui.get_library_tab() == 4
            && c.storage_ready.get()
            && ui.get_selected_track() == self.id
            && library.selected_id() == self.id
            && c.library_generation.get() == self.generation
            && library.revision() == self.revision
            && library
                .track_by_id(self.id)
                .is_some_and(|t| t.path == self.path)
    }
}

impl AudioController {
    pub(super) fn wire_metadata_diagnosis(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_metadata_diagnosis(move || {
            if let Some(controller) = weak.upgrade() {
                controller.open_metadata_diagnosis();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_close_metadata_diagnosis(move || {
            if let Some(controller) = weak.upgrade() {
                controller.close_metadata_diagnosis();
            }
        });
    }

    fn open_metadata_diagnosis(&self) {
        self.close_metadata_diagnosis();
        let Some(ui) = self.ui.upgrade() else { return };
        if ui.get_library_tab() != 4 || !self.storage_ready.get() {
            ui.set_status("Open Prepare and wait for the library to finish loading".into());
            return;
        }
        let id = ui.get_selected_track();
        let library = self.library.borrow();
        let Some(track) = library
            .track_by_id(id)
            .filter(|_| id > 0 && library.selected_id() == id)
        else {
            ui.set_status("Select a track in Prepare first".into());
            return;
        };
        let source = Source {
            id,
            path: track.path.clone(),
            generation: self.library_generation.get(),
            revision: library.revision(),
        };
        let catalog = CatalogValues::from_track(track);
        drop(library);
        self.close_comparison();
        self.close_reference_editor();
        {
            let mut state = self.diagnosis.borrow_mut();
            state.source = Some(source);
            state.pending = Some(catalog);
        }
        ui.set_metadata_diagnosis(crate::MetadataDiagnosisView {
            open: true,
            track_id: id,
            text: "Reading file tags…".into(),
            compare: false,
        });
        self.poll_metadata_diagnosis();
    }

    pub(super) fn close_metadata_diagnosis(&self) {
        let mut state = self.diagnosis.borrow_mut();
        if state.source.take().is_none() {
            return;
        }
        if let Some(job) = &state.job {
            job.cancel.request();
        }
        state.pending = None;
        drop(state);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_metadata_diagnosis(Default::default());
        }
    }

    pub(super) fn poll_metadata_diagnosis(&self) {
        let state = self.diagnosis.borrow();
        if state.source.is_none() && state.job.is_none() {
            return;
        }
        let ui = self.ui.upgrade();
        let stale = state
            .source
            .as_ref()
            .is_some_and(|source| ui.as_ref().is_none_or(|ui| !source.is_current(self, ui)));
        drop(state);
        if stale {
            self.close_metadata_diagnosis();
        }
        let completed = {
            let mut state = self.diagnosis.borrow_mut();
            state
                .poll()
                .and_then(|result| state.source.as_ref().map(|s| (s.id, result)))
        };
        if let (Some(ui), Some((id, result))) = (ui, completed) {
            let (text, compare) = match result {
                Ok(diagnosis) => (diagnosis.text, diagnosis.compare),
                Err(error) => (error, false),
            };
            ui.set_metadata_diagnosis(crate::MetadataDiagnosisView {
                open: true,
                track_id: id,
                text: text.into(),
                compare,
            });
        }
    }
}
