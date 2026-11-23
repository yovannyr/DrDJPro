//! Prepare clip diagnosis owns one background job, one selected passage and the PFL voice.
use super::*;
use crate::{
    cancellation::Cancellation,
    playback::{
        clip_diagnosis::{self, ClipReport},
        PreviewClip,
    },
};

struct Session {
    id: i32,
    path: PathBuf,
    generation: u64,
    report: Option<Arc<ClipReport>>,
    marker: Option<usize>,
    pair: Option<(PreviewClip, PreviewClip)>,
    audition: Option<bool>,
}

impl Session {
    fn current(&self, c: &AudioController, ui: &AppWindow) -> bool {
        if !ui.get_clip_check_visible()
            || ui.get_selected_track() != self.id
            || !c.storage_ready.get()
            || c.library_generation.get() != self.generation
        {
            return false;
        }
        let library = c.library.borrow();
        self.id == library.selected_id()
            && library
                .track_by_id(self.id)
                .is_some_and(|t| t.path == self.path)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Scan,
    Compare,
    Export,
}

enum Work {
    Scan,
    Compare(Arc<ClipReport>, usize, Option<(PreviewClip, PreviewClip)>),
    Export(Arc<ClipReport>, LibraryFolderPicker),
}
enum Output {
    Scanned(ClipReport),
    Pair(usize, PreviewClip, PreviewClip),
    Exported(Option<PathBuf>),
}

impl Work {
    fn kind(&self) -> Kind {
        match self {
            Self::Scan => Kind::Scan,
            Self::Compare(..) => Kind::Compare,
            Self::Export(..) => Kind::Export,
        }
    }
    fn run(self, path: &Path, cancel: &Cancellation) -> Result<Output, String> {
        cancel.check()?;
        match self {
            Self::Scan => clip_diagnosis::scan(path, cancel).map(Output::Scanned),
            Self::Compare(report, index, cached) => {
                let (original, processed) = if let Some(pair) = cached {
                    report.verify(path, cancel)?;
                    pair
                } else {
                    clip_diagnosis::comparison(path, &report, index, cancel)?
                };
                Ok(Output::Pair(index, original, processed))
            }
            Self::Export(report, picker) => {
                let dir = picker();
                cancel.check()?;
                dir.map(|dir| crate::clip_export::export(path, &report, &dir, cancel))
                    .transpose()
                    .map(Output::Exported)
            }
        }
    }
}

struct Job {
    kind: Kind,
    cancel: Cancellation,
    receiver: Receiver<Result<Output, String>>,
}
impl Job {
    fn start(work: Work, path: PathBuf) -> Result<Self, String> {
        let kind = work.kind();
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (tx, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("clip-diagnosis".into())
            .spawn(move || {
                let _ = tx.send(work.run(&path, &token));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            kind,
            cancel,
            receiver,
        })
    }
}

#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    pending: Option<Work>,
    job: Option<Job>,
}

impl State {
    fn running(&self, kind: Kind) -> bool {
        self.pending.as_ref().is_some_and(|p| p.kind() == kind)
            || self
                .job
                .as_ref()
                .is_some_and(|job| job.kind == kind && !job.cancel.is_cancelled())
    }

    fn cancel(&mut self) {
        self.pending = None;
        if let Some(job) = &self.job {
            job.cancel.request();
        }
    }
    fn stop_audio(&mut self, c: &AudioController) {
        if let Some(session) = &mut self.session {
            session.audition = None;
            c.engine.borrow().stop_tempo_preview();
        }
        if self.running(Kind::Compare) {
            self.cancel();
        }
    }
    fn poll(&mut self) -> Option<Result<Output, String>> {
        if let Some(job) = &self.job {
            let result = match job.receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err("Clip diagnosis worker stopped.".into()),
            };
            let stale = job.cancel.is_cancelled();
            self.job = None;
            if !stale {
                return Some(result);
            }
        }
        if let (Some(session), Some(work)) = (&self.session, self.pending.take()) {
            match Job::start(work, session.path.clone()) {
                Ok(job) => self.job = Some(job),
                Err(e) => return Some(Err(e)),
            }
        }
        None
    }
}

impl AudioController {
    pub(super) fn wire_clip_check(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_clip_check(move || {
            if let Some(c) = weak.upgrade() {
                c.open_clip_check();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_close_clip_check(move || {
            if let Some(c) = weak.upgrade() {
                c.close_clip_check();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_select_clip_marker(move |index| {
            if let Some(c) = weak.upgrade() {
                c.select_clip_marker(index);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_audition_clip_check(move |processed| {
            if let Some(c) = weak.upgrade() {
                c.audition_clip_check(processed);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_stop_clip_check(move || {
            if let Some(c) = weak.upgrade() {
                c.stop_clip_check();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_export_clip_check(move || {
            if let Some(c) = weak.upgrade() {
                c.export_clip_check();
            }
        });
    }

    fn open_clip_check(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let id = ui.get_selected_track();
        let library = self.library.borrow();
        if !self.storage_ready.get()
            || ui.get_library_tab() != 4
            || library.selected_id() != id
            || ui.get_import_review_open()
        {
            return;
        }
        let Some(track) = library.track_by_id(id) else {
            ui.set_status("Select a track in Prepare first.".into());
            return;
        };
        let path = track.path.clone();
        drop(library);
        self.close_clip_check();
        self.close_comparison();
        self.close_reference_editor();
        self.close_sample_browser();
        self.close_metadata_diagnosis();
        let mut state = self.clip_check.borrow_mut();
        state.session = Some(Session {
            id,
            path,
            generation: self.library_generation.get(),
            report: None,
            marker: None,
            pair: None,
            audition: None,
        });
        state.pending = Some(Work::Scan);
        drop(state);
        ui.set_clip_check_track(id);
        ui.set_clip_check_index(-1);
        ui.set_clip_check_route(self.engine.borrow().preview_route_available());
        ui.set_clip_check_open(true);
        ui.set_clip_check_status("Scanning the complete track for flattened peaks…".into());
        self.poll_clip_check();
    }

    pub(super) fn close_clip_check(&self) {
        let mut state = self.clip_check.borrow_mut();
        if state.session.is_none() {
            return;
        }
        state.stop_audio(self);
        state.cancel();
        state.session = None;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_clip_check_open(false);
            ui.set_clip_check_index(-1);
            ui.set_clip_check_track(-1);
            ui.set_clip_check_busy(false);
            ui.set_clip_check_route(false);
            ui.set_clip_check_markers(Default::default());
            ui.set_clip_check_status(Default::default());
        }
    }

    fn clip_check_window(&self) -> Option<AppWindow> {
        let ui = self.ui.upgrade()?;
        self.clip_check
            .borrow()
            .session
            .as_ref()
            .is_some_and(|s| s.current(self, &ui))
            .then_some(ui)
    }

    fn select_clip_marker(&self, index: i32) {
        let Some(ui) = self.clip_check_window() else {
            return;
        };
        let mut state = self.clip_check.borrow_mut();
        if state.running(Kind::Export) {
            return;
        }
        let Some(index) = usize::try_from(index).ok().filter(|&i| {
            state
                .session
                .as_ref()
                .and_then(|s| s.report.as_ref())
                .is_some_and(|r| i < r.markers.len())
        }) else {
            return;
        };
        state.stop_audio(self);
        if let Some(session) = &mut state.session {
            if session.marker != Some(index) {
                session.pair = None;
            }
            session.marker = Some(index);
        }
        ui.set_clip_check_index(index as i32);
        ui.set_clip_check_busy(false);
        ui.set_clip_check_status("Compare the original and processed passage.".into());
    }

    fn audition_clip_check(&self, processed: bool) {
        let Some(ui) = self.clip_check_window() else {
            return;
        };
        let mut state = self.clip_check.borrow_mut();
        if state.running(Kind::Export) {
            return;
        }
        if !self.engine.borrow().preview_route_available()
            || self.pending_device_switch.borrow().is_some()
        {
            state.stop_audio(self);
            ui.set_clip_check_route(false);
            ui.set_clip_check_status(
                "A separate headphone output (3/4) is required to audition.".into(),
            );
            return;
        }
        state.stop_audio(self);
        let Some(s) = &mut state.session else { return };
        let (Some(report), Some(index)) = (&s.report, s.marker) else {
            return;
        };
        s.audition = Some(processed);
        state.pending = Some(Work::Compare(report.clone(), index, s.pair.take()));
        ui.set_clip_check_status("Checking the source for headphone comparison…".into());
        drop(state);
        self.poll_clip_check();
    }

    fn play_clip_pair(&self, ui: &AppWindow, clip: &PreviewClip, processed: bool) {
        let status = match self.engine.borrow().play_sample_preview(clip) {
            Ok(()) if processed => "Playing processed passage on headphones.".into(),
            Ok(()) => "Playing original passage on headphones.".into(),
            Err(e) => e,
        };
        ui.set_clip_check_status(status.into());
    }

    fn stop_clip_check(&self) {
        let Some(ui) = self.clip_check_window() else {
            return;
        };
        let mut state = self.clip_check.borrow_mut();
        state.stop_audio(self);
        if !state.running(Kind::Scan) && !state.running(Kind::Export) {
            ui.set_clip_check_busy(false);
            ui.set_clip_check_status("Headphone comparison stopped.".into());
        }
    }

    fn export_clip_check(&self) {
        let Some(ui) = self.clip_check_window() else {
            return;
        };
        let mut state = self.clip_check.borrow_mut();
        if state.running(Kind::Export) {
            return;
        }
        let Some(report) = state
            .session
            .as_ref()
            .and_then(|s| s.report.clone())
            .filter(|r| !r.markers.is_empty())
        else {
            return;
        };
        let Some(picker) = self.library_folder_picker.borrow().clone() else {
            ui.set_clip_check_status("Export folder picker unavailable.".into());
            return;
        };
        state.stop_audio(self);
        state.pending = Some(Work::Export(report, picker));
        ui.set_clip_check_status("Choose a folder for the separate processed WAV copy.".into());
        drop(state);
        self.poll_clip_check();
    }

    pub(super) fn poll_clip_check(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if self
            .clip_check
            .borrow()
            .session
            .as_ref()
            .is_some_and(|s| !s.current(self, &ui))
        {
            self.close_clip_check();
        }
        let mut state = self.clip_check.borrow_mut();
        if state.session.is_none() && state.job.is_none() {
            return;
        }
        let route = self.engine.borrow().preview_route_available()
            && self.pending_device_switch.borrow().is_none();
        if state.session.is_some() && route != ui.get_clip_check_route() {
            ui.set_clip_check_route(route);
            if !route {
                state.stop_audio(self);
                ui.set_clip_check_status("Headphone output unavailable.".into());
            }
        }
        let done = state.poll();
        if state.session.is_none() {
            return;
        }
        let busy = state.pending.is_some()
            || state
                .job
                .as_ref()
                .is_some_and(|job| !job.cancel.is_cancelled());
        ui.set_clip_check_busy(busy);
        let Some(result) = done else { return };
        let Some(session) = &mut state.session else {
            return;
        };
        match result {
            Ok(Output::Scanned(report)) => {
                let list = report
                    .markers
                    .iter()
                    .map(|m| {
                        slint::SharedString::from(format!(
                            "{:.2} – {:.2} s",
                            m.start.as_secs_f64(),
                            m.end.as_secs_f64()
                        ))
                    })
                    .collect::<Vec<_>>();
                let count = list.len();
                let seconds = report.duration.as_secs_f64();
                session.marker = if count > 0 { Some(0) } else { None };
                session.report = Some(Arc::new(report));
                ui.set_clip_check_markers(ModelRc::new(VecModel::from(list)));
                ui.set_clip_check_index(if count > 0 { 0 } else { -1 });
                ui.set_clip_check_status(if count==0{format!("No suspected clipped passages in {seconds:.1} s.")}else{format!("{count} suspected clipped passages in {seconds:.1} s. Select a passage to compare.")}.into());
            }
            Ok(Output::Pair(index, a, b)) if session.marker == Some(index) => {
                if let Some(processed) = session.audition.take().filter(|_| route) {
                    self.play_clip_pair(&ui, if processed { &b } else { &a }, processed);
                }
                session.pair = Some((a, b));
            }
            Ok(Output::Pair(..)) => (),
            Ok(Output::Exported(Some(path))) => ui.set_clip_check_status(
                format!("Saved separate WAV copy: {}", path.display()).into(),
            ),
            Ok(Output::Exported(None)) => ui.set_clip_check_status("Export cancelled.".into()),
            Err(error) => {
                session.audition = None;
                ui.set_clip_check_status(error.into());
            }
        }
    }
}
