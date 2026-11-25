use super::{local_keys::Version, *};
use crate::{
    cancellation::Cancellation,
    waveform_layers::{analyze, Levels},
};

#[cfg(test)]
mod acceptance;

enum Update {
    Loading,
    Ready(Vec<Levels>),
    Error(String),
}
struct Job {
    id: i32,
    path: PathBuf,
    cancel: Cancellation,
    rx: Receiver<Update>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.request();
    }
}
#[derive(Default)]
pub(super) struct State {
    job: Option<Job>,
    shown: i32,
    failed: Option<(i32, PathBuf)>,
}

fn watch(path: &Path, cancel: &Cancellation, tx: &mpsc::SyncSender<Update>) {
    let mut seen = None;
    let mut last_error = None;
    while !cancel.is_cancelled() {
        let result = Version::read(path, cancel).and_then(|stamp| {
            if seen.as_ref() == Some(&stamp) {
                return Ok(());
            }
            if last_error.is_none() {
                tx.send(Update::Loading).map_err(|e| e.to_string())?;
            }
            let levels = analyze(path, cancel)?;
            if Version::read(path, cancel)? != stamp {
                return Err("File changed during analysis.".into());
            }
            cancel.check()?;
            tx.send(Update::Ready(levels)).map_err(|e| e.to_string())?;
            seen = Some(stamp);
            Ok(())
        });
        match result {
            Err(e) if !cancel.is_cancelled() => {
                seen = None;
                if last_error.as_ref() != Some(&e) {
                    if tx.send(Update::Error(e.clone())).is_err() {
                        break;
                    }
                    last_error = Some(e);
                }
            }
            Ok(()) => last_error = None,
            _ => break,
        }
        let waits = if matches!(seen, Some(Version::Native(_))) {
            10
        } else {
            120
        };
        for _ in 0..waits {
            if cancel.is_cancelled() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

impl AudioController {
    pub(super) fn invalidate_layer_selection(&self, ui: &AppWindow, selected: i32) {
        let mut state = self.layer_view.borrow_mut();
        if state.shown == 0 && state.job.is_none() && state.failed.is_none() {
            return;
        }
        if let Some(job) = &state.job {
            job.cancel.request();
        }
        state.shown = 0;
        state.failed = None;
        clear_layers(ui, selected > 0);
    }

    pub(super) fn poll_layer_view(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let wanted = ui.get_layer_view_active();
        let id = if wanted { ui.get_selected_track() } else { 0 };
        let mut state = self.layer_view.borrow_mut();
        if id == 0 && state.job.is_none() && state.shown == 0 && state.failed.is_none() {
            return;
        }
        let lib = self.library.borrow();
        let path = (id > 0 && lib.selected_id() == id)
            .then(|| lib.track_by_id(id))
            .flatten()
            .filter(|t| t.available)
            .map(|t| t.path.as_path());
        if state.shown != id || path.is_none() {
            clear_layers(&ui, path.is_some());
            state.shown = id;
            state.failed = None;
        }
        if let Some(job) = &state.job {
            if id != job.id || path != Some(job.path.as_path()) {
                job.cancel.request();
                ui.set_layer_percussion(ModelRc::default());
                ui.set_layer_tonal(ModelRc::default());
            }
            match job.rx.try_recv() {
                Ok(message) if !job.cancel.is_cancelled() => match message {
                    Update::Ready(values) => {
                        ui.set_layer_percussion(ModelRc::new(VecModel::from(
                            values.iter().map(|v| v.percussion).collect::<Vec<_>>(),
                        )));
                        ui.set_layer_tonal(ModelRc::new(VecModel::from(
                            values.iter().map(|v| v.tonal).collect::<Vec<_>>(),
                        )));
                        ui.set_layer_status("Percussion above · tonal below".into());
                    }
                    Update::Loading => {
                        ui.set_layer_percussion(ModelRc::default());
                        ui.set_layer_tonal(ModelRc::default());
                        ui.set_layer_status("Preparing waveform…".into());
                    }
                    Update::Error(e) => {
                        ui.set_layer_percussion(ModelRc::default());
                        ui.set_layer_tonal(ModelRc::default());
                        ui.set_layer_status(e.into());
                    }
                },
                Err(TryRecvError::Disconnected) => {
                    state.job = None;
                }
                _ => {}
            }
        }
        let Some(path) = path else {
            return;
        };
        if state.job.is_some()
            || state
                .failed
                .as_ref()
                .is_some_and(|(failed, p)| *failed == id && p == path)
        {
            return;
        }
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let source = path.to_owned();
        let (tx, rx) = mpsc::sync_channel(2);
        match std::thread::Builder::new()
            .name("prepare-waveform-layers".into())
            .spawn(move || watch(&source, &token, &tx))
        {
            Ok(_) => {
                state.job = Some(Job {
                    id,
                    path: path.to_owned(),
                    cancel,
                    rx,
                })
            }
            Err(e) => {
                state.failed = Some((id, path.to_owned()));
                ui.set_layer_status(e.to_string().into());
            }
        }
    }
}

fn clear_layers(ui: &AppWindow, available: bool) {
    ui.set_layer_percussion(ModelRc::default());
    ui.set_layer_tonal(ModelRc::default());
    ui.set_layer_status(
        if available {
            "Preparing waveform…"
        } else {
            "Choose an available track."
        }
        .into(),
    );
}
