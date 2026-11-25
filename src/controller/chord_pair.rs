//! Two independent Prepare selections share an output-time axis. Corrections retain source time.
mod midi_export;
mod projection;
use super::local_keys::Version;
use super::*;
use crate::{
    cancellation::Cancellation,
    chord_passage::{self, ChordEvent, ChordProfile},
    chord_timing::PairTiming,
    library_store::ChordConfirmations,
    playback::PreviewClip,
};
use projection::{Cell, ModelRows, Row};
use std::time::Instant;

#[derive(Clone)]
struct Pin {
    id: i32,
    path: PathBuf,
}
#[derive(Clone)]
struct Layer {
    pin: Pin,
    version: Arc<Version>,
    profile: Arc<ChordProfile>,
    cells: Vec<Cell>,
}

impl Layer {
    fn new(
        pin: Pin,
        window: Option<(u64, u64)>,
        store: Option<&ChordConfirmations>,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        let before = Version::read(&pin.path, cancel)?;
        let profile = match window {
            Some((start, end)) => chord_passage::analyze(&pin.path, start, end - start, cancel)?,
            None => ChordProfile {
                source_sha256: crate::local_harmony::fingerprint(&pin.path, cancel)?,
                events: Vec::new(),
            },
        };
        let mut layer = Self {
            pin,
            version: Arc::new(before),
            profile: Arc::new(profile),
            cells: Vec::new(),
        };
        layer.reload(window, store)?;
        layer.verify(cancel)?;
        Ok(layer)
    }
    fn reload(
        &mut self,
        window: Option<(u64, u64)>,
        store: Option<&ChordConfirmations>,
    ) -> Result<(), String> {
        if let Some((start, end)) = window {
            let confirmed = store
                .map(|s| {
                    s.lookup(
                        self.pin.id,
                        &self.pin.path,
                        &self.profile.source_sha256,
                        start,
                        end,
                    )
                })
                .transpose()?
                .unwrap_or_default();
            self.cells = projection::layer_cells(&self.profile.events, &confirmed, start, end);
        }
        Ok(())
    }
    fn verify(&self, cancel: &Cancellation) -> Result<(), String> {
        if Version::read(&self.pin.path, cancel)? != *self.version {
            return Err("A source file changed. Compare the pinned tracks again.".into());
        }
        Ok(())
    }
}

struct Snapshot {
    layers: [Layer; 2],
    timing: PairTiming,
    rows: Vec<Row>,
}
impl Snapshot {
    fn new(layers: [Layer; 2], timing: PairTiming) -> Self {
        let rows = projection::rows(&layers, timing);
        Self {
            layers,
            timing,
            rows,
        }
    }
    fn verify(&self, cancel: &Cancellation) -> Result<(), String> {
        for l in &self.layers {
            l.verify(cancel)?;
        }
        Ok(())
    }
    fn interval(&self) -> Duration {
        if self
            .layers
            .iter()
            .all(|l| matches!(l.version.as_ref(), Version::Native(_)))
        {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(30)
        }
    }
}

enum Work {
    Compare([Pin; 2], PairTiming, Option<ChordConfirmations>),
    Verify(Arc<Snapshot>),
    Audition(Arc<Snapshot>),
    Confirm(Arc<Snapshot>, usize, ChordEvent, ChordConfirmations),
    Export(midi_export::Request),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Compare,
    Verify,
    Audition,
    Confirm,
    Export,
}
enum Done {
    Pair(Arc<Snapshot>),
    Verified,
    Audio(PreviewClip),
    Exported(Option<PathBuf>),
}
impl Work {
    fn kind(&self) -> Kind {
        match self {
            Self::Compare(..) => Kind::Compare,
            Self::Verify(_) => Kind::Verify,
            Self::Audition(_) => Kind::Audition,
            Self::Confirm(..) => Kind::Confirm,
            Self::Export(_) => Kind::Export,
        }
    }
    fn run(self, cancel: &Cancellation) -> Result<Done, String> {
        cancel.check()?;
        match self {
            Self::Export(req) => req.run(cancel).map(Done::Exported),
            Self::Compare([a, b], t, store) => {
                let a = Layer::new(a, t.source_window(0), store.as_ref(), cancel)?;
                let b = Layer::new(b, t.source_window(1), store.as_ref(), cancel)?;
                a.verify(cancel)?;
                Ok(Done::Pair(Arc::new(Snapshot::new([a, b], t))))
            }
            Self::Verify(pair) => {
                pair.verify(cancel)?;
                Ok(Done::Verified)
            }
            Self::Audition(pair) => {
                pair.verify(cancel)?;
                let clip = crate::playback::chord_preview::render(
                    &pair.layers[0].pin.path,
                    &pair.layers[1].pin.path,
                    pair.timing,
                    cancel,
                )?;
                pair.verify(cancel)?;
                Ok(Done::Audio(clip))
            }
            Self::Confirm(pair, side, event, store) => {
                pair.verify(cancel)?;
                let l = &pair.layers[side];
                cancel.check()?;
                store.confirm(l.pin.id, &l.pin.path, &l.profile.source_sha256, &event)?;
                let mut layers = pair.layers.clone();
                for (lane, layer) in layers.iter_mut().enumerate() {
                    if layer.pin.id == l.pin.id {
                        layer.reload(pair.timing.source_window(lane), Some(&store))?;
                    }
                }
                pair.verify(cancel)?;
                Ok(Done::Pair(Arc::new(Snapshot::new(layers, pair.timing))))
            }
        }
    }
}

struct Job {
    kind: Kind,
    cancel: Cancellation,
    rx: Receiver<Result<Done, String>>,
}
impl Job {
    fn start(work: Work) -> Result<Self, String> {
        let kind = work.kind();
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("chord-compare".into())
            .spawn(move || {
                let _ = tx.send(work.run(&token));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { kind, cancel, rx })
    }
}
struct Session {
    pins: [Option<Pin>; 2],
    pair: Option<Arc<Snapshot>>,
    index: Option<usize>,
    verify_at: Instant,
    generation: u64,
    grid_revision: Option<u64>,
}
#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    pending: Option<Work>,
    job: Option<Job>,
    export_guard: Option<(i32, Arc<crate::beatgrid::BeatGrid>)>,
}
impl State {
    fn cancel(&mut self) {
        self.export_guard = None;
        self.pending = None;
        if let Some(job) = &self.job {
            job.cancel.request();
        }
    }
    fn running(&self, kind: Kind) -> bool {
        self.pending.as_ref().is_some_and(|w| w.kind() == kind)
            || self
                .job
                .as_ref()
                .is_some_and(|j| j.kind == kind && !j.cancel.is_cancelled())
    }
    fn poll(&mut self) -> Option<(Kind, Result<Done, String>)> {
        if let Some(j) = &self.job {
            let r = match j.rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err("Chord comparison worker stopped.".into()),
            };
            let kind = j.kind;
            let stale = j.cancel.is_cancelled();
            self.job = None;
            if !stale {
                return Some((kind, r));
            }
        }
        if let Some(work) = self.pending.take() {
            let kind = work.kind();
            match Job::start(work) {
                Ok(job) => self.job = Some(job),
                Err(e) => return Some((kind, Err(e))),
            }
        }
        None
    }
}

fn clear_rows(ui: &AppWindow) {
    ui.set_chord_pair_index(-1);
    ui.set_chord_pair_rows(Default::default());
}
impl AudioController {
    pub(super) fn wire_chord_pair(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_export_chord_midi(move |side| {
            if let Some(c) = weak.upgrade() {
                c.export_chord_midi(side);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_open_chord_pair(move || {
            if let Some(c) = weak.upgrade() {
                c.open_chord_pair();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_close_chord_pair(move || {
            if let Some(c) = weak.upgrade() {
                c.close_chord_pair();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_use_chord_track(move |side| {
            if let Some(c) = weak.upgrade() {
                c.use_chord_track(side);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_compare_chord_pair(move |settings| {
            if let Some(c) = weak.upgrade() {
                c.compare_chord_pair(settings);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_select_chord_row(move |index| {
            if let Some(c) = weak.upgrade() {
                c.select_chord_row(index);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_confirm_chord_event(move |side, label| {
            if let Some(c) = weak.upgrade() {
                c.confirm_chord_event(side, &label);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_audition_chord_pair(move || {
            if let Some(c) = weak.upgrade() {
                c.audition_chord_pair();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_stop_chord_pair(move || {
            if let Some(c) = weak.upgrade() {
                c.stop_chord_pair();
            }
        });
    }
    fn open_chord_pair(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        if ui.get_library_tab() != 4 || ui.get_import_review_open() || !self.storage_ready.get() {
            return;
        }
        self.close_local_keys();
        self.close_clip_check();
        self.close_comparison();
        self.close_reference_editor();
        self.close_sample_browser();
        self.close_metadata_diagnosis();
        self.close_chord_pair();
        self.chord_pair.borrow_mut().session = Some(Session {
            pins: [None, None],
            pair: None,
            index: None,
            verify_at: Instant::now(),
            generation: self.library_generation.get(),
            grid_revision: None,
        });
        ui.set_chord_pair_route(self.engine.borrow().preview_route_available());
        ui.set_chord_pair_open(true);
        ui.set_chord_pair_status("Select a Library track and choose Use A or Use B.".into());
    }
    pub(super) fn close_chord_pair(&self) {
        let mut st = self.chord_pair.borrow_mut();
        if st.session.take().is_none() {
            return;
        }
        st.cancel();
        self.engine.borrow().stop_tempo_preview();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_chord_pair_open(false);
            clear_rows(&ui);
            ui.set_chord_pair_a(Default::default());
            ui.set_chord_pair_b(Default::default());
            ui.set_chord_pair_busy(false);
            ui.set_chord_pair_route(false);
            ui.set_chord_pair_status(Default::default());
        }
    }
    fn pair_ui(&self) -> Option<AppWindow> {
        let ui = self.ui.upgrade()?;
        ui.get_chord_pair_visible().then_some(ui)
    }
    fn use_chord_track(&self, side: i32) {
        let Some(ui) = self.pair_ui() else { return };
        let Ok(side) = usize::try_from(side) else {
            return;
        };
        if side > 1 {
            return;
        }
        let lib = self.library.borrow();
        let id = ui.get_selected_track();
        let Some(track) = lib.track_by_id(id).filter(|_| lib.selected_id() == id) else {
            ui.set_chord_pair_status("Select a track in the Library first.".into());
            return;
        };
        let pin = Pin {
            id,
            path: track.path.clone(),
        };
        let name = track.display_title().to_owned();
        drop(lib);
        let mut st = self.chord_pair.borrow_mut();
        st.cancel();
        let Some(s) = &mut st.session else { return };
        s.pins[side] = Some(pin);
        s.pair = None;
        s.index = None;
        self.engine.borrow().stop_tempo_preview();
        clear_rows(&ui);
        ui.set_chord_pair_busy(false);
        if side == 0 {
            ui.set_chord_pair_a(name.into())
        } else {
            ui.set_chord_pair_b(name.into())
        }
        ui.set_chord_pair_status("Choose the other track or compare the pinned passages.".into());
    }
    fn compare_chord_pair(&self, settings: crate::ChordPairSettings) {
        let Some(ui) = self.pair_ui() else { return };
        let mut st = self.chord_pair.borrow_mut();
        st.cancel();
        self.engine.borrow().stop_tempo_preview();
        let Some(s) = &mut st.session else { return };
        s.pair = None;
        s.index = None;
        clear_rows(&ui);
        ui.set_chord_pair_busy(false);
        let [Some(a), Some(b)] = &s.pins else {
            ui.set_chord_pair_status("Pin both Library tracks first.".into());
            return;
        };
        let timing = match projection::settings(settings) {
            Ok(t) => t,
            Err(e) => {
                ui.set_chord_pair_status(e.into());
                return;
            }
        };
        let store = self
            .persistence
            .borrow()
            .as_ref()
            .map(|w| w.chord_confirmations());
        st.pending = Some(Work::Compare([a.clone(), b.clone()], timing, store));
        ui.set_chord_pair_status("Analyzing both source passages…".into());
        drop(st);
        self.poll_chord_pair();
    }
    fn select_chord_row(&self, index: i32) {
        let Some(ui) = self.pair_ui() else { return };
        let Ok(i) = usize::try_from(index) else {
            return;
        };
        let mut st = self.chord_pair.borrow_mut();
        let Some(s) = &mut st.session else { return };
        if s.pair.as_ref().is_some_and(|p| i < p.rows.len()) {
            s.index = Some(i);
            ui.set_chord_pair_index(index);
        }
    }
    fn confirm_chord_event(&self, side: i32, label: &str) {
        let Some(ui) = self.pair_ui() else { return };
        let Ok(side) = usize::try_from(side) else {
            return;
        };
        if side > 1 {
            return;
        }
        let mut st = self.chord_pair.borrow_mut();
        if st.running(Kind::Confirm) {
            return;
        }
        let Some((pair, index)) = st
            .session
            .as_ref()
            .and_then(|s| Some((s.pair.clone()?, s.index?)))
        else {
            return;
        };
        let Some(cell) = pair.rows[index].cells[side].map(|i| &pair.layers[side].cells[i]) else {
            ui.set_chord_pair_status(
                "This side has no source event in the selected interval.".into(),
            );
            return;
        };
        let Some(store) = self
            .persistence
            .borrow()
            .as_ref()
            .map(|w| w.chord_confirmations())
        else {
            ui.set_chord_pair_status("Library storage is unavailable.".into());
            return;
        };
        let mut event = cell.event.clone();
        let raw = label.trim();
        if !raw.is_empty() {
            event.chord = if raw.eq_ignore_ascii_case("unknown") {
                None
            } else if let Some(chord) = chord_passage::canonical_chord(raw) {
                Some(chord)
            } else {
                ui.set_chord_pair_status("Enter a major or minor source chord, or Unknown.".into());
                return;
            };
        }
        event.confidence = if event.chord.is_some() { 100 } else { 0 };
        st.cancel();
        self.engine.borrow().stop_tempo_preview();
        st.pending = Some(Work::Confirm(pair, side, event, store));
        ui.set_chord_pair_status("Saving the confirmed source interval…".into());
        drop(st);
        self.poll_chord_pair();
    }
    fn audition_chord_pair(&self) {
        let Some(ui) = self.pair_ui() else { return };
        let mut st = self.chord_pair.borrow_mut();
        if st.running(Kind::Confirm) {
            return;
        }
        let Some(pair) = st.session.as_ref().and_then(|s| s.pair.clone()) else {
            return;
        };
        st.cancel();
        self.engine.borrow().stop_tempo_preview();
        if !self.engine.borrow().preview_route_available()
            || self.pending_device_switch.borrow().is_some()
        {
            ui.set_chord_pair_status("A separate headphone output (3/4) is required.".into());
            return;
        }
        st.pending = Some(Work::Audition(pair));
        ui.set_chord_pair_status("Preparing both passages for headphones…".into());
        drop(st);
        self.poll_chord_pair();
    }
    fn stop_chord_pair(&self) {
        let Some(ui) = self.pair_ui() else { return };
        let mut st = self.chord_pair.borrow_mut();
        if st.session.is_none() {
            return;
        }
        self.engine.borrow().stop_tempo_preview();
        if st.running(Kind::Audition) || st.running(Kind::Export) {
            st.cancel();
            ui.set_chord_pair_busy(false);
        }
        if !st.running(Kind::Compare) && !st.running(Kind::Confirm) {
            ui.set_chord_pair_status("Headphone comparison stopped.".into());
        }
    }
    pub(super) fn poll_chord_pair(&self) {
        {
            let st = self.chord_pair.borrow();
            if st.session.is_none() && st.job.is_none() {
                return;
            }
        }
        let Some(ui) = self.ui.upgrade() else { return };
        let stale = self.chord_pair.borrow().session.as_ref().is_some_and(|s| {
            if !ui.get_chord_pair_visible()
                || !self.storage_ready.get()
                || self.library_generation.get() != s.generation
            {
                return true;
            }
            let lib = self.library.borrow();
            s.pins
                .iter()
                .flatten()
                .any(|p| lib.track_by_id(p.id).is_none_or(|t| t.path != p.path))
        });
        if stale {
            self.close_chord_pair();
        }
        let mut st = self.chord_pair.borrow_mut();
        if st.session.is_some() {
            if st.export_guard.as_ref().is_some_and(|(id, grid)| {
                self.library
                    .borrow()
                    .track_by_id(*id)
                    .and_then(|t| t.grid)
                    .as_ref()
                    != Some(grid.as_ref())
            }) {
                st.cancel();
                ui.set_chord_pair_status(
                    "Source grid changed. Choose the MIDI export again.".into(),
                );
            }
            let route = self.engine.borrow().preview_route_available()
                && self.pending_device_switch.borrow().is_none();
            if route != ui.get_chord_pair_route() {
                ui.set_chord_pair_route(route);
                if !route {
                    self.engine.borrow().stop_tempo_preview();
                    if st.running(Kind::Audition) {
                        st.cancel();
                    }
                    ui.set_chord_pair_status("Separate headphone output unavailable.".into());
                }
            }
            if st.job.is_none() && st.pending.is_none() {
                if let Some(pair) = st
                    .session
                    .as_ref()
                    .filter(|s| Instant::now() >= s.verify_at)
                    .and_then(|s| s.pair.clone())
                {
                    st.pending = Some(Work::Verify(pair));
                }
            }
        }
        let done = st.poll();
        if done.as_ref().is_some_and(|(kind, _)| *kind == Kind::Export) {
            st.export_guard = None;
        }
        if st.session.is_none() {
            return;
        }
        ui.set_chord_pair_busy(
            st.pending
                .as_ref()
                .is_some_and(|w| w.kind() != Kind::Verify)
                || st
                    .job
                    .as_ref()
                    .is_some_and(|j| j.kind != Kind::Verify && !j.cancel.is_cancelled()),
        );
        let Some(s) = &mut st.session else { return };
        if let Some((kind, result)) = done {
            match result {
                Ok(Done::Pair(pair)) => {
                    s.index = if pair.rows.is_empty() {
                        None
                    } else {
                        Some(if kind == Kind::Confirm {
                            s.index.unwrap_or(0).min(pair.rows.len() - 1)
                        } else {
                            0
                        })
                    };
                    s.verify_at = Instant::now() + pair.interval();
                    s.pair = Some(pair);
                    s.grid_revision = None;
                    ui.set_chord_pair_index(s.index.map_or(-1, |v| v as i32));
                    ui.set_chord_pair_status(if kind == Kind::Confirm {
                        "Source chord confirmed and saved.".into()
                    } else {
                        format!(
                            "Compared both source passages with chord analysis v{}.",
                            chord_passage::ALGORITHM_VERSION
                        )
                        .into()
                    });
                }
                Ok(Done::Verified) => {
                    if let Some(p) = &s.pair {
                        s.verify_at = Instant::now() + p.interval();
                    }
                }
                Ok(Done::Audio(clip)) => {
                    match self.engine.borrow().play_sample_preview(&clip) {
                        Ok(()) => ui.set_chord_pair_status(
                            "Playing aligned passages on headphones.".into(),
                        ),
                        Err(e) => ui.set_chord_pair_status(e.into()),
                    };
                }
                Ok(Done::Exported(path)) => ui.set_chord_pair_status(path.map_or_else(
                    || "MIDI export cancelled.".into(),
                    |p| format!("MIDI saved to {}", p.display()).into(),
                )),
                Err(e) if kind == Kind::Export => ui.set_chord_pair_status(e.into()),
                Err(e) => {
                    s.pair = None;
                    s.index = None;
                    self.engine.borrow().stop_tempo_preview();
                    clear_rows(&ui);
                    ui.set_chord_pair_status(e.into());
                }
            }
        }
        if let Some(p) = &s.pair {
            let lib = self.library.borrow();
            if s.grid_revision != Some(lib.revision()) {
                let grids = std::array::from_fn(|side| {
                    lib.track_by_id(p.layers[side].pin.id).and_then(|t| t.grid)
                });
                ui.set_chord_pair_rows(ModelRc::new(ModelRows {
                    pair: Arc::clone(p),
                    grids,
                    notify: Default::default(),
                }));
                s.grid_revision = Some(lib.revision());
            }
        }
    }
}
