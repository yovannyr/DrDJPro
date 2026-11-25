//! Frame-addressed Prepare cue drafts. The monitor and library commit never seek either deck.
use super::local_keys::Version;
use super::*;
use crate::{
    beatgrid::BeatGrid,
    cancellation::Cancellation,
    hot_cues::{CueTrimChange, HotCue, HotCues},
    playback::PreviewClip,
};
use std::collections::HashMap;

fn time(frame: u64, rate: u32) -> Duration {
    let ns = (u128::from(frame) * 1_000_000_000).div_ceil(u128::from(rate));
    Duration::from_nanos(ns.min(u128::from(u64::MAX)) as u64)
}
fn frame(t: Duration, rate: u32) -> u64 {
    (t.as_nanos() * u128::from(rate) / 1_000_000_000).min(u128::from(u64::MAX)) as u64
}
#[derive(Clone, Copy, PartialEq)]
struct Basis {
    saved: Option<BeatGrid>,
    tempo: Option<BeatGrid>,
    grid: bool,
}
impl Basis {
    fn from_track(t: &crate::library::Track) -> Self {
        if let Some(g) = t.grid {
            return Self {
                saved: Some(g),
                tempo: Some(g),
                grid: true,
            };
        }
        let analysis = t.metadata.analysis.as_ref();
        let anchor = analysis.and_then(|a| a.beat_anchor_ms);
        let tempo = analysis.and_then(|a| {
            BeatGrid::new(
                f64::from(a.bpm),
                Duration::from_millis(u64::from(anchor.unwrap_or(0))),
            )
            .ok()
        });
        Self {
            saved: None,
            tempo,
            grid: tempo.is_some() && anchor.is_some(),
        }
    }
}
#[derive(Clone)]
enum Reference {
    Free,
    Grid,
    Cue(usize, HotCue),
}
impl Reference {
    fn index(&self) -> i32 {
        match self {
            Self::Free => -2,
            Self::Grid => -1,
            Self::Cue(i, _) => *i as i32,
        }
    }
}
struct Draft {
    slot: usize,
    original: HotCue,
    position: u64,
    reference: Reference,
}
struct Info {
    rate: u32,
    last: u64,
    version: Arc<Version>,
}
struct Session {
    id: i32,
    path: PathBuf,
    generation: u64,
    revision: u64,
    slots: HotCues,
    basis: Basis,
    info: Option<Info>,
    draft: Option<Draft>,
    recent: Vec<(usize, Duration)>,
    wait_save: bool,
}
impl Session {
    fn suggested(&self, slot: usize) -> Option<usize> {
        let slots = self.slots.slots();
        let target = slots.get(slot)?.as_ref()?.position();
        self.recent
            .iter()
            .find_map(|(i, at)| {
                (*i != slot && slots[*i].as_ref().is_some_and(|c| c.position() == *at))
                    .then_some(*i)
            })
            .or_else(|| {
                slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        let at = c.as_ref()?.position();
                        (i != slot && at <= target).then_some((at, i))
                    })
                    .max()
                    .map(|(_, i)| i)
            })
    }
    fn choose_slot(&mut self, slot: usize) -> Result<(), String> {
        let info = self
            .info
            .as_ref()
            .ok_or("Wait for the source frame count.")?;
        let cue = self
            .slots
            .slots()
            .get(slot)
            .and_then(Option::as_ref)
            .ok_or("Select an occupied cue slot.")?
            .clone();
        let reference = if self.basis.grid {
            Reference::Grid
        } else {
            Reference::Free
        };
        self.draft = Some(Draft {
            slot,
            position: frame(cue.position(), info.rate).min(info.last),
            original: cue,
            reference,
        });
        Ok(())
    }
    fn step(&mut self, at: u64, direction: i32) -> Result<(), String> {
        let info = self.info.as_ref().ok_or("Source frames unavailable.")?;
        let d = self.draft.as_mut().ok_or("Select an occupied cue.")?;
        let at = at.min(info.last);
        d.position = match &d.reference {
            Reference::Free => at
                .saturating_add_signed(i64::from(direction))
                .min(info.last),
            reference => {
                let grid = self.basis.tempo.ok_or("No local tempo. Choose Free.")?;
                let phase = if let Reference::Cue(_, cue) = reference {
                    Some(cue.position())
                } else {
                    None
                };
                grid.cue_quarter_frame(time(at, info.rate), phase, direction, info.rate, info.last)?
            }
        };
        Ok(())
    }
    fn choose_reference(&mut self, which: i32) -> Result<(), String> {
        let d = self.draft.as_ref().ok_or("Select an occupied cue.")?;
        let reference = match which {
            -2 => Reference::Free,
            -1 if self.basis.grid => Reference::Grid,
            -1 => return Err(
                "No saved grid or analyzed downbeat. Choose Free or another cue with local tempo."
                    .into(),
            ),
            i if (0..8).contains(&i) => {
                let index = i as usize;
                if index == d.slot {
                    return Err("Choose a different cue as reference.".into());
                }
                let cue = self.slots.slots()[index]
                    .as_ref()
                    .ok_or("The reference slot is empty.")?;
                if self.basis.tempo.is_none() {
                    return Err("No local tempo. Free frame editing remains available.".into());
                }
                Reference::Cue(index, cue.clone())
            }
            _ => return Err("Invalid cue reference.".into()),
        };
        let at = d.position;
        self.draft.as_mut().expect("selected draft").reference = reference;
        self.step(at, 0)
    }
    fn change(&self) -> Result<CueTrimChange, String> {
        let d = self.draft.as_ref().ok_or("Select a cue to save.")?;
        let info = self.info.as_ref().ok_or("Source frames unavailable.")?;
        let reference = match &d.reference {
            Reference::Cue(i, cue) => Some((*i, cue.clone())),
            _ => None,
        };
        Ok(CueTrimChange {
            slot: d.slot,
            original: d.original.clone(),
            reference,
            expected_grid: (!matches!(d.reference, Reference::Free)).then_some(self.basis.saved),
            position: time(d.position, info.rate),
        })
    }
    fn publish(&self, ui: &AppWindow, notice: &str) {
        ui.set_cue_trim_slot(self.draft.as_ref().map_or(-1, |d| d.slot as i32));
        ui.set_cue_trim_reference(self.draft.as_ref().map_or(-2, |d| d.reference.index()));
        let Some((d, i)) = self.draft.as_ref().zip(self.info.as_ref()) else {
            ui.set_cue_trim_suggested(-1);
            ui.set_cue_trim_position(Default::default());
            ui.set_cue_trim_status(notice.into());
            return;
        };
        let suggested = self.suggested(d.slot);
        ui.set_cue_trim_suggested(suggested.map_or(-1, |s| s as i32));
        ui.set_cue_trim_position(format!("{:.9}", time(d.position, i.rate).as_secs_f64()).into());
        let mode = match &d.reference {
            Reference::Free => "Free · 1 source frame".to_string(),
            Reference::Grid => "Beatgrid · ¼ beat".into(),
            Reference::Cue(slot, _) => format!("Cue {} · ¼ beat", slot + 1),
        };
        let suggestion = suggested
            .map(|s| format!(" · Suggested cue {}", s + 1))
            .unwrap_or_default();
        ui.set_cue_trim_status(format!("{notice} {mode}{suggestion}").trim().into());
    }
    fn publish_slots(&self, ui: &AppWindow) {
        let rows = self
            .slots
            .slots()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                slint::SharedString::from(match c {
                    Some(cue) => format!(
                        "{} · {} · {:.6} s",
                        i + 1,
                        cue.name(),
                        cue.position().as_secs_f64()
                    ),
                    None => format!("{} · Empty", i + 1),
                })
            })
            .collect::<Vec<_>>();
        ui.set_cue_trim_slots(ModelRc::new(VecModel::from(rows)));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inspect,
    Audition,
    Save,
}
enum Work {
    Inspect,
    Audition(Arc<Version>, u64),
    Save(Arc<Version>),
}

#[cfg(test)]
mod qualification {
    use super::*;
    #[test]
    fn d05_cue_relative_grid_and_free_frame_bounds_do_not_mutate_saved_slots() {
        let ms = Duration::from_millis;
        let rate = 48_000;
        let last = rate as u64 * 2 - 1;
        let grid = BeatGrid::new(120., Duration::ZERO).unwrap();
        let mut slots = HotCues::default();
        for i in 0..2 {
            slots
                .set(
                    i,
                    Some(HotCue::new(ms(1030), format!("cue {i}"), 0).unwrap()),
                )
                .unwrap();
        }
        let mut session = Session {
            id: 1,
            path: "track.wav".into(),
            generation: 1,
            revision: 0,
            slots,
            basis: Basis {
                saved: Some(grid),
                tempo: Some(grid),
                grid: true,
            },
            info: Some(Info {
                rate,
                last,
                version: Arc::new(Version::Digest([0; 32])),
            }),
            draft: None,
            recent: Vec::new(),
            wait_save: false,
        };
        session.choose_slot(1).unwrap();
        session.choose_reference(0).unwrap();
        session.step(frame(ms(1030), rate), 1).unwrap();
        assert_eq!(
            time(session.draft.as_ref().unwrap().position, rate),
            ms(1155)
        );
        session.choose_reference(-1).unwrap();
        session.step(frame(ms(1030), rate), 1).unwrap();
        assert_eq!(
            time(session.draft.as_ref().unwrap().position, rate),
            ms(1125)
        );
        session.choose_reference(-2).unwrap();
        for (at, direction, expected) in [(0, -1, 0), (last, 1, last)] {
            session.step(at, direction).unwrap();
            assert_eq!(session.draft.as_ref().unwrap().position, expected);
        }
        assert!(session.choose_reference(3).is_err());
        for cue in &session.slots.slots()[..2] {
            assert_eq!(cue.as_ref().unwrap().position(), ms(1030));
        }
    }
}
enum ResultData {
    Info(Info),
    Audio(PreviewClip),
    Checked,
}
impl Work {
    fn kind(&self) -> Kind {
        match self {
            Self::Inspect => Kind::Inspect,
            Self::Audition(..) => Kind::Audition,
            Self::Save(_) => Kind::Save,
        }
    }
    fn perform(self, path: &Path, c: &Cancellation) -> Result<ResultData, String> {
        let check = |version: &Version| -> Result<(), String> {
            if Version::read(path, c)? != *version {
                return Err("The audio file changed. Reopen cue trim.".into());
            }
            Ok(())
        };
        match self {
            Self::Inspect => {
                let version = Version::read(path, c)?;
                let (rate, count) = crate::playback::cue_trim::source_frames(path, c)?;
                if rate == 0 || count == 0 {
                    return Err("Source has no audio frames.".into());
                }
                check(&version)?;
                Ok(ResultData::Info(Info {
                    rate,
                    last: count - 1,
                    version: Arc::new(version),
                }))
            }
            Self::Audition(v, f) => {
                check(&v)?;
                let clip = crate::playback::cue_trim::audition(path, f, c)?;
                check(&v)?;
                Ok(ResultData::Audio(clip))
            }
            Self::Save(v) => {
                check(&v)?;
                Ok(ResultData::Checked)
            }
        }
    }
}
struct Job {
    kind: Kind,
    cancel: Cancellation,
    rx: Receiver<Result<ResultData, String>>,
}
impl Job {
    fn start(path: PathBuf, work: Work) -> Result<Self, String> {
        let kind = work.kind();
        let cancel = Cancellation::default();
        let c = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("prepare-cue-trim".into())
            .spawn(move || {
                let _ = tx.send(work.perform(&path, &c));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { kind, cancel, rx })
    }
}
#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    pending: Option<Work>,
    job: Option<Job>,
    moved: HashMap<i32, Vec<(usize, Duration)>>,
    generation: u64,
}
impl State {
    fn cancel(&mut self) {
        self.pending = None;
        if let Some(j) = &self.job {
            j.cancel.request();
        }
        if let Some(s) = &mut self.session {
            s.wait_save = false;
        }
    }
    fn cancel_action(&mut self) {
        if !matches!(self.pending, Some(Work::Inspect)) {
            self.pending = None;
        }
        if let Some(j) = self.job.as_ref().filter(|j| j.kind != Kind::Inspect) {
            j.cancel.request();
        }
        if let Some(s) = &mut self.session {
            s.wait_save = false;
        }
    }
    fn poll(&mut self) -> Option<Result<ResultData, String>> {
        if let Some(j) = &self.job {
            let result = match j.rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err("Cue trim worker stopped.".into()),
            };
            let stale = j.cancel.is_cancelled();
            self.job = None;
            if !stale {
                return Some(result);
            }
        }
        if let (Some(s), Some(w)) = (&self.session, self.pending.take()) {
            match Job::start(s.path.clone(), w) {
                Ok(j) => self.job = Some(j),
                Err(e) => return Some(Err(e)),
            }
        }
        None
    }
}
enum Edit {
    Slot(i32),
    Reference(i32),
    Move(i32),
    Time(slint::SharedString),
}
impl AudioController {
    pub(super) fn wire_cue_trim(self: &Rc<Self>, ui: &AppWindow) {
        let w = Rc::downgrade(self);
        ui.on_open_cue_trim(move || {
            if let Some(c) = w.upgrade() {
                c.open_cue_trim();
            }
        });
        let w = Rc::downgrade(self);
        ui.on_close_cue_trim(move || {
            if let Some(c) = w.upgrade() {
                c.close_cue_trim();
            }
        });
        let w = Rc::downgrade(self);
        ui.on_select_cue_trim_slot(move |v| {
            if let Some(c) = w.upgrade() {
                c.edit_cue_trim(Edit::Slot(v));
            }
        });
        let w = Rc::downgrade(self);
        ui.on_select_cue_trim_reference(move |v| {
            if let Some(c) = w.upgrade() {
                c.edit_cue_trim(Edit::Reference(v));
            }
        });
        let w = Rc::downgrade(self);
        ui.on_move_cue_trim(move |v| {
            if let Some(c) = w.upgrade() {
                c.edit_cue_trim(Edit::Move(v));
            }
        });
        let w = Rc::downgrade(self);
        ui.on_set_cue_trim_position(move |v| {
            if let Some(c) = w.upgrade() {
                c.edit_cue_trim(Edit::Time(v));
            }
        });
        let w = Rc::downgrade(self);
        ui.on_audition_cue_trim(move || {
            if let Some(c) = w.upgrade() {
                c.trim_action(false);
            }
        });
        let w = Rc::downgrade(self);
        ui.on_save_cue_trim(move || {
            if let Some(c) = w.upgrade() {
                c.trim_action(true);
            }
        });
        let w = Rc::downgrade(self);
        ui.on_stop_cue_trim(move || {
            if let Some(c) = w.upgrade() {
                c.stop_trim_audition();
            }
        });
    }
    fn open_cue_trim(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let id = ui.get_selected_track();
        let lib = self.library.borrow();
        if ui.get_library_tab() != 4
            || !self.storage_ready.get()
            || lib.selected_id() != id
            || ui.get_import_review_open()
        {
            return;
        }
        let Some(t) = lib.track_by_id(id) else {
            ui.set_status("Select a track in Prepare.".into());
            return;
        };
        let generation = self.library_generation.get();
        let mut s = Session {
            id,
            path: t.path.clone(),
            generation,
            revision: lib.revision(),
            slots: t.hot_cues.clone(),
            basis: Basis::from_track(t),
            info: None,
            draft: None,
            recent: Vec::new(),
            wait_save: false,
        };
        drop(lib);
        self.close_cue_trim();
        self.close_chord_pair();
        self.close_comparison();
        self.close_reference_editor();
        self.close_sample_browser();
        self.close_clip_check();
        self.close_local_keys();
        self.close_metadata_diagnosis();
        let mut st = self.cue_trim.borrow_mut();
        if st.generation != generation {
            st.moved.clear();
            st.generation = generation;
        }
        s.recent = st.moved.get(&id).cloned().unwrap_or_default();
        s.publish_slots(&ui);
        s.publish(&ui, "Reading exact source frame count…");
        st.session = Some(s);
        st.pending = Some(Work::Inspect);
        drop(st);
        ui.set_cue_trim_track(id);
        ui.set_cue_trim_open(true);
        ui.set_cue_trim_route(self.engine.borrow().preview_route_available());
        self.poll_cue_trim();
    }
    pub(super) fn close_cue_trim(&self) {
        let mut st = self.cue_trim.borrow_mut();
        if st.session.take().is_none() {
            return;
        }
        st.cancel();
        self.engine.borrow().stop_tempo_preview();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_cue_trim_open(false);
            ui.set_cue_trim_busy(false);
            ui.set_cue_trim_route(false);
            ui.set_cue_trim_track(-1);
            ui.set_cue_trim_slot(-1);
            ui.set_cue_trim_suggested(-1);
            ui.set_cue_trim_reference(-2);
            ui.set_cue_trim_position(Default::default());
            ui.set_cue_trim_slots(Default::default());
            ui.set_cue_trim_status(Default::default());
        }
    }
    fn trim_ui(&self) -> Option<AppWindow> {
        let ui = self.ui.upgrade()?;
        ui.get_cue_trim_visible().then_some(ui)
    }
    fn edit_cue_trim(&self, edit: Edit) {
        self.poll_cue_trim();
        let Some(ui) = self.trim_ui() else { return };
        let mut st = self.cue_trim.borrow_mut();
        st.cancel_action();
        self.engine.borrow().stop_tempo_preview();
        let Some(s) = &mut st.session else { return };
        let result = match edit {
            Edit::Slot(i) => usize::try_from(i)
                .map_err(|_| "Invalid cue slot.".into())
                .and_then(|i| s.choose_slot(i)),
            Edit::Reference(i) => s.choose_reference(i),
            Edit::Move(n) if matches!(n, -1 | 1) => s
                .draft
                .as_ref()
                .map(|d| d.position)
                .ok_or_else(|| "Select a cue.".to_string())
                .and_then(|f| s.step(f, n)),
            Edit::Move(_) => Err("Choose one step left or right.".into()),
            Edit::Time(text) => text
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(|v| Duration::try_from_secs_f64(v).ok())
                .ok_or_else(|| "Enter non-negative seconds.".to_string())
                .and_then(|t| {
                    let rate = s.info.as_ref().ok_or("Source frames unavailable.")?.rate;
                    s.step(frame(t, rate), 0)
                }),
        };
        s.publish(&ui, result.err().as_deref().unwrap_or(""));
    }
    fn stop_trim_audition(&self) {
        let Some(ui) = self.trim_ui() else { return };
        let mut st = self.cue_trim.borrow_mut();
        if matches!(st.pending, Some(Work::Audition(..))) {
            st.pending = None;
        }
        if let Some(j) = st.job.as_ref().filter(|j| j.kind == Kind::Audition) {
            j.cancel.request();
        }
        self.engine.borrow().stop_tempo_preview();
        if let Some(s) = &st.session {
            s.publish(&ui, "Headphone preview stopped.");
        }
    }
    fn trim_action(&self, save: bool) {
        self.poll_cue_trim();
        let Some(ui) = self.trim_ui() else { return };
        let mut st = self.cue_trim.borrow_mut();
        st.cancel_action();
        self.engine.borrow().stop_tempo_preview();
        let Some(s) = &mut st.session else { return };
        let Some((d, info)) = s.draft.as_ref().zip(s.info.as_ref()) else {
            return;
        };
        if save {
            if d.position == frame(d.original.position(), info.rate) {
                s.publish(&ui, "No position change to save.");
                return;
            }
            s.wait_save = true;
            s.publish(&ui, "Saving cue…");
        } else {
            if !self.engine.borrow().preview_route_available()
                || self.pending_device_switch.borrow().is_some()
            {
                s.publish(&ui, "A separate headphone output (3/4) is required.");
                return;
            }
            let work = Work::Audition(Arc::clone(&info.version), d.position);
            s.publish(&ui, "Preparing headphone preview…");
            st.pending = Some(work);
        }
        drop(st);
        self.poll_cue_trim();
    }

    fn commit_trim(&self) -> Result<(), String> {
        let st = self.cue_trim.borrow();
        let s = st.session.as_ref().ok_or("Cue editor closed.")?;
        let change = s.change()?;
        let lib = self.library.borrow();
        let track = lib.track_by_id(s.id).ok_or("Track no longer available.")?;
        if !matches!(
            s.draft.as_ref().map(|d| &d.reference),
            Some(Reference::Free)
        ) && Basis::from_track(track) != s.basis
        {
            return Err("Local tempo changed. Select the reference again.".into());
        }
        lib.validate_cue_trim(s.id, &s.path, &change)?;
        drop(lib);
        self.persistence
            .borrow()
            .as_ref()
            .ok_or("Library unavailable for saving.")?
            .commit_cue_trim(s.id, &s.path, &change)?;
        self.library
            .borrow_mut()
            .apply_committed_cue_trim(s.id, &s.path, &change)?;
        let id = s.id;
        drop(st);
        self.cue_targets_changed();
        let mut st = self.cue_trim.borrow_mut();
        let history = st.moved.entry(id).or_default();
        history.retain(|(slot, _)| *slot != change.slot);
        history.insert(0, (change.slot, change.position));
        history.truncate(2);
        Ok(())
    }
    pub(super) fn poll_cue_trim(&self) {
        {
            let st = self.cue_trim.borrow();
            if st.session.is_none() && st.job.is_none() {
                return;
            }
        }
        let Some(ui) = self.ui.upgrade() else { return };
        let obsolete = self.cue_trim.borrow().session.as_ref().is_some_and(|s| {
            if !ui.get_cue_trim_visible()
                || !self.storage_ready.get()
                || self.library_generation.get() != s.generation
            {
                return true;
            }
            let l = self.library.borrow();
            l.selected_id() != s.id || l.track_by_id(s.id).is_none_or(|t| t.path != s.path)
        });
        if obsolete {
            self.close_cue_trim();
        }
        let mut st = self.cue_trim.borrow_mut();
        let mut cancel_action = false;
        if let Some(s) = &mut st.session {
            let lib = self.library.borrow();
            if lib.revision() != s.revision {
                s.revision = lib.revision();
                if let Some(track) = lib.track_by_id(s.id) {
                    let basis = Basis::from_track(track);
                    let old_basis = s.basis;
                    s.basis = basis;
                    let changed = s.slots != track.hot_cues;
                    s.slots = track.hot_cues.clone();
                    if changed {
                        s.publish_slots(&ui);
                    }
                    if let Some(d) = &mut s.draft {
                        if s.slots.slots()[d.slot].as_ref() != Some(&d.original) {
                            s.draft = None;
                            cancel_action = true;
                            s.publish(&ui, "Target cue changed. Select an occupied slot again.");
                        } else if (!matches!(d.reference, Reference::Free) && old_basis != basis)
                            || matches!(&d.reference,Reference::Cue(i,old) if s.slots.slots()[*i].as_ref()!=Some(old))
                        {
                            d.reference = Reference::Free;
                            cancel_action = true;
                            s.publish(&ui, "Reference changed. Draft position retained.");
                        } else if changed {
                            s.publish(&ui, "");
                        }
                    }
                }
            }
        }
        if cancel_action {
            st.cancel_action();
            self.engine.borrow().stop_tempo_preview();
        }
        let route = self.engine.borrow().preview_route_available()
            && self.pending_device_switch.borrow().is_none();
        if st.session.is_some() && route != ui.get_cue_trim_route() {
            ui.set_cue_trim_route(route);
            if !route {
                self.engine.borrow().stop_tempo_preview();
                if matches!(st.pending, Some(Work::Audition(..))) {
                    st.pending = None;
                }
                if let Some(j) = st.job.as_ref().filter(|j| j.kind == Kind::Audition) {
                    j.cancel.request();
                }
            }
        }
        if st.session.as_ref().is_some_and(|s| s.wait_save) {
            if self.save_failed.get() {
                if let Some(s) = &mut st.session {
                    s.wait_save = false;
                    s.publish(&ui, "Resolve the pending Library save first.");
                }
            } else if self.dirty.get() || self.saving.get() {
                self.save_library();
            } else if let Some(s) = &mut st.session {
                s.wait_save = false;
                st.pending = s.info.as_ref().map(|i| Work::Save(Arc::clone(&i.version)));
            }
        }
        let done = st.poll();
        let busy = st.pending.is_some()
            || st.job.as_ref().is_some_and(|j| !j.cancel.is_cancelled())
            || st.session.as_ref().is_some_and(|s| s.wait_save);
        let Some(s) = &mut st.session else { return };
        ui.set_cue_trim_busy(busy);
        match done {
            Some(Ok(ResultData::Info(info))) => {
                s.info = Some(info);
                if let Some(index) = s.slots.slots().iter().position(Option::is_some) {
                    if let Err(e) = s.choose_slot(index) {
                        s.publish(&ui, &e);
                        return;
                    }
                    s.publish(&ui, "");
                } else {
                    s.publish(&ui, "Set a hot cue on this track before trimming it.");
                }
            }
            Some(Ok(ResultData::Audio(clip))) => {
                let result = self.engine.borrow().play_sample_preview(&clip);
                s.publish(
                    &ui,
                    result
                        .err()
                        .as_deref()
                        .unwrap_or("Playing from the draft frame."),
                );
            }
            Some(Ok(ResultData::Checked)) => {
                if self.saving.get() || self.dirty.get() {
                    s.wait_save = true;
                    ui.set_cue_trim_busy(true);
                    return;
                }
                drop(st);
                match self.commit_trim() {
                    Ok(()) => {
                        self.close_cue_trim();
                        ui.set_status("Cue position saved.".into());
                    }
                    Err(e) => {
                        if let Some(s) = &self.cue_trim.borrow().session {
                            s.publish(&ui, &e);
                        }
                    }
                }
            }
            Some(Err(e)) => s.publish(&ui, &e),
            None => (),
        }
    }
}
