//! Selecting a file only auditions it. Slot replacement uses the regular sampler loader on Assign.
use super::tempo_comparison::{PreviewJob, PreviewOwner};
use super::*;
use crate::{cancellation::Cancellation, playback::SamplePreviewCache};
use std::path::Path;

const MAX_FOLDER_FILES: usize = 4096;
const NO_ROUTE: &str =
    "No separate headphone output (3/4). You can still assign the selected file.";

struct FolderJob {
    cancel: Cancellation,
    result: Receiver<Result<Option<Vec<PathBuf>>, String>>,
}

impl FolderJob {
    fn start(picker: LibraryFolderPicker) -> Result<Self, String> {
        let cancel = Cancellation::default();
        let worker = cancel.clone();
        let (tx, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("sample-folder".into())
            .spawn(move || {
                let found = picker().map(|p| list_audio_files(&p, &worker)).transpose();
                let _ = tx.send(found);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { cancel, result })
    }
}

fn list_audio_files(dir: &Path, cancel: &Cancellation) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        cancel.check()?;
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if crate::audio_formats::supported(&path)
            && entry.file_type().map_err(|e| e.to_string())?.is_file()
        {
            if files.len() == MAX_FOLDER_FILES {
                return Err(format!("This folder has more than {MAX_FOLDER_FILES} audio files. Choose a smaller folder."));
            }
            files.push(path);
        }
    }
    cancel.check()?;
    files.sort_unstable();
    Ok(files)
}

#[derive(Default)]
pub(super) struct Browser {
    slot: Option<usize>,
    files: Vec<PathBuf>,
    selected: Option<usize>,
    epoch: u64,
    wants_audio: bool,
    cache: SamplePreviewCache,
    folder: Option<FolderJob>,
    choose_folder: bool,
}

impl Browser {
    fn stop(&mut self, c: &AudioController) {
        self.wants_audio = false;
        self.epoch = self.epoch.wrapping_add(1);
        c.cancel_preview_job(PreviewOwner::Sample);
        c.engine.borrow().stop_tempo_preview();
    }

    fn clear_folder(&mut self, c: &AudioController, ui: &AppWindow) {
        self.stop(c);
        if let Some(job) = &self.folder {
            job.cancel.request();
        }
        self.choose_folder = false;
        self.files = Vec::new();
        self.selected = None;
        self.cache.clear();
        ui.set_samp_preview_files(Default::default());
        ui.set_samp_preview_index(-1);
        ui.set_samp_preview_bytes(0);
        ui.set_samp_preview_busy(false);
    }

    fn poll_folder(&mut self, c: &AudioController, ui: &AppWindow) {
        if let Some(job) = &self.folder {
            let finished = match job.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("Folder picker stopped.".into())),
            };
            if let Some(result) = finished {
                let cancelled = job.cancel.is_cancelled();
                self.folder = None;
                if !cancelled && self.slot.is_some() {
                    ui.set_samp_preview_busy(false);
                    let status = match result {
                        Ok(Some(files)) => {
                            let names = files
                                .iter()
                                .filter_map(|p| p.file_name())
                                .map(|n| slint::SharedString::from(n.to_string_lossy().as_ref()))
                                .collect::<Vec<_>>();
                            self.files = files;
                            let count = self.files.len();
                            ui.set_samp_preview_files(ModelRc::new(VecModel::from(names)));
                            if count == 0 {
                                "No supported audio files in this folder.".into()
                            } else if !ui.get_samp_preview_route() {
                                NO_ROUTE.into()
                            } else {
                                format!("{count} audio files. Select a sample to audition.")
                            }
                        }
                        Ok(None) => "Folder selection cancelled.".into(),
                        Err(e) => format!("Cannot list samples: {e}"),
                    };
                    ui.set_samp_preview_status(status.into());
                }
            }
        }
        if self.slot.is_some() && self.choose_folder && self.folder.is_none() {
            self.choose_folder = false;
            let picker = c.library_folder_picker.borrow().clone();
            match picker
                .ok_or_else(|| "Folder picker unavailable.".to_string())
                .and_then(FolderJob::start)
            {
                Ok(folder) => self.folder = Some(folder),
                Err(e) => {
                    ui.set_samp_preview_busy(false);
                    ui.set_samp_preview_status(e.into());
                }
            }
        }
    }
}

impl AudioController {
    pub(super) fn wire_sample_browser(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_sample_browser(move |slot| {
            if let Some(c) = weak.upgrade() {
                c.open_sample_browser(slot);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_close_sample_browser(move || {
            if let Some(c) = weak.upgrade() {
                c.close_sample_browser();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_choose_sample_folder(move || {
            if let Some(c) = weak.upgrade() {
                c.choose_sample_folder();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_select_sample_preview(move |index| {
            if let Some(c) = weak.upgrade() {
                c.select_sample_preview(index);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_assign_sample_preview(move || {
            if let Some(c) = weak.upgrade() {
                c.assign_sample_preview();
            }
        });
    }

    fn open_sample_browser(&self, slot: i32) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_sampler_open()
            || !self.storage_ready.get()
            || !(0..4).contains(&slot)
            || self.pending_device_switch.borrow().is_some()
        {
            return;
        }
        self.close_comparison();
        self.close_reference_editor();
        self.close_sample_browser();
        self.sample_browser.borrow_mut().slot = Some(slot as usize);
        let route = self.engine.borrow().preview_route_available();
        ui.set_samp_preview_slot(slot);
        ui.set_samp_preview_index(-1);
        ui.set_samp_preview_route(route);
        ui.set_samp_preview_status(
            if route {
                "Choose a folder to browse local samples."
            } else {
                NO_ROUTE
            }
            .into(),
        );
        ui.set_samp_preview_open(true);
    }

    pub(super) fn close_sample_browser(&self) {
        let mut b = self.sample_browser.borrow_mut();
        if b.slot.take().is_none() {
            return;
        }
        if let Some(ui) = self.ui.upgrade() {
            b.clear_folder(self, &ui);
            ui.set_samp_preview_open(false);
            ui.set_samp_preview_route(false);
            ui.set_samp_preview_status(Default::default());
        } else {
            b.stop(self);
            if let Some(job) = &b.folder {
                job.cancel.request();
            }
        }
    }

    fn choose_sample_folder(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let mut b = self.sample_browser.borrow_mut();
        if b.slot.is_none() || !ui.get_sampler_open() || ui.get_samp_preview_busy() {
            return;
        }
        b.clear_folder(self, &ui);
        b.choose_folder = true;
        ui.set_samp_preview_busy(true);
        ui.set_samp_preview_status("Choosing a sample folder…".into());
        b.poll_folder(self, &ui);
    }

    fn select_sample_preview(&self, index: i32) {
        let Some(ui) = self.ui.upgrade() else { return };
        let mut b = self.sample_browser.borrow_mut();
        if b.slot.is_none() || !ui.get_sampler_open() || ui.get_samp_preview_busy() {
            return;
        }
        let Some(i) = usize::try_from(index).ok().filter(|&i| i < b.files.len()) else {
            return;
        };
        b.stop(self);
        b.selected = Some(i);
        ui.set_samp_preview_index(index);
        b.wants_audio = self.engine.borrow().preview_route_available()
            && self.pending_device_switch.borrow().is_none();
        ui.set_samp_preview_route(b.wants_audio);
        ui.set_samp_preview_status(
            if b.wants_audio {
                "Loading headphone preview…"
            } else {
                NO_ROUTE
            }
            .into(),
        );
        drop(b);
        self.poll_sample_browser();
    }

    fn assign_sample_preview(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_sampler_open() || ui.get_samp_preview_busy() {
            return;
        }
        let b = self.sample_browser.borrow();
        let (Some(slot), Some(path)) = (b.slot, b.selected.and_then(|i| b.files.get(i)).cloned())
        else {
            return;
        };
        drop(b);
        let result = self.load_sampler_slot(
            &mut self.sampler.borrow_mut(),
            slot,
            Arc::new(move || Some(path.clone())),
        );
        match result {
            Ok(()) => self.close_sample_browser(),
            Err(e) => ui.set_samp_preview_status(e.into()),
        }
    }

    pub(super) fn poll_sample_browser(&self) {
        let done = self.take_preview_result(PreviewOwner::Sample);
        let Some(ui) = self.ui.upgrade() else { return };
        if self.sample_browser.borrow().slot.is_some()
            && (!ui.get_sampler_open()
                || !ui.get_samp_preview_open()
                || self.pending_device_switch.borrow().is_some())
        {
            self.close_sample_browser();
        }
        let mut b = self.sample_browser.borrow_mut();
        b.poll_folder(self, &ui);
        if b.slot.is_none() {
            return;
        }
        let route = self.engine.borrow().preview_route_available();
        if route != ui.get_samp_preview_route() {
            ui.set_samp_preview_route(route);
            if route {
                ui.set_samp_preview_status("Select a sample to audition on headphones.".into());
            } else {
                b.stop(self);
                ui.set_samp_preview_status(NO_ROUTE.into());
            }
        }
        if !route || !b.wants_audio {
            return;
        }
        let Some(index) = b.selected else { return };
        if let Some((epoch, false, result)) = done {
            if b.epoch == epoch {
                b.wants_audio = false;
                match result.and_then(|clip| {
                    self.engine.borrow().play_sample_preview(&clip)?;
                    let path = b.files[index].clone();
                    b.cache.insert(path, clip);
                    Ok(())
                }) {
                    Ok(()) => {
                        ui.set_samp_preview_bytes(b.cache.bytes() as i32);
                        ui.set_samp_preview_status(
                            "Headphone preview. Assign to replace this slot.".into(),
                        );
                    }
                    Err(e) => ui.set_samp_preview_status(e.into()),
                }
                return;
            }
        }
        if self.preview_job.borrow().is_none() {
            let path = b.files[index].clone();
            let cached = b.cache.get(&path);
            match PreviewJob::sample(path, b.epoch, cached) {
                Ok(job) => *self.preview_job.borrow_mut() = Some(job),
                Err(e) => {
                    b.wants_audio = false;
                    ui.set_samp_preview_status(e.into());
                }
            }
        }
    }
}
