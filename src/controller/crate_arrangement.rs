//! Ordinary-crate order previews. Only the explicit Apply command writes to storage.
use super::{set_planning::Lane, *};
use crate::{
    chord_timing::PairTiming,
    crate_order::{self, OrderTrack},
    playback::PreviewClip,
    CrateOrderWindow, OrderAnchorRow,
};
use std::collections::HashMap;

fn strings(values: impl IntoIterator<Item = String>) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(
        values.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

struct Source {
    ids: Vec<i32>,
    tracks: Arc<Vec<OrderTrack>>,
    positions: HashMap<i32, usize>,
    fixed: Vec<bool>,
}
impl Source {
    fn from_library(lib: &crate::library::Library, id: i32, ids: Vec<i32>) -> Result<Self, String> {
        if !same_members(lib, id, &ids) {
            return Err("The crate membership changed. Reload the crate.".into());
        }
        let tracks = ids
            .iter()
            .map(|&id| {
                let t = lib.planning_track(id).ok_or("A track left the library.")?;
                Ok(OrderTrack {
                    id,
                    name: t.name,
                    bpm: t.bpm,
                    key: t.key,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let positions = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let fixed = vec![false; ids.len()];
        Ok(Self {
            ids,
            tracks: Arc::new(tracks),
            positions,
            fixed,
        })
    }
    fn track(&self, id: i32) -> &OrderTrack {
        &self.tracks[self.positions[&id]]
    }
    fn show_anchors(&self, v: &CrateOrderWindow) {
        v.set_anchors(ModelRc::new(VecModel::from(
            self.tracks
                .iter()
                .enumerate()
                .map(|(i, t)| OrderAnchorRow {
                    title: format!("{}. {}", i + 1, t.name).into(),
                    fixed: self.fixed[i],
                })
                .collect::<Vec<_>>(),
        )));
    }
}
fn same_members(lib: &crate::library::Library, id: i32, ids: &[i32]) -> bool {
    lib.active_crate() == id
        && lib.crates().entries.iter().any(|c| {
            c.id == id
                && c.smart_query.is_none()
                && c.tracks.len() == ids.len()
                && ids.iter().all(|track| c.tracks.contains(track))
        })
}

enum Work {
    Loaded(Vec<i32>),
    Variants(Vec<Vec<i32>>),
}
struct Session {
    view: CrateOrderWindow,
    generation: u64,
    crate_id: i32,
    revision: u64,
    source: Option<Source>,
    variants: Vec<Vec<i32>>,
    variant: Option<usize>,
    hard: Vec<usize>,
    transition: Option<usize>,
}
impl Session {
    fn clear_variants(&mut self) {
        self.variants.clear();
        self.variant = None;
        self.hard.clear();
        self.transition = None;
        self.view.set_variants(Default::default());
        self.view.set_preview(Default::default());
        self.view.set_transitions(Default::default());
        self.view.set_variant_index(-1);
        self.view.set_transition_index(-1);
        self.view.set_can_audition(false);
    }
    fn choose(&mut self, i: usize) {
        let (Some(s), Some(order)) = (&self.source, self.variants.get(i)) else {
            return;
        };
        self.variant = Some(i);
        self.view.set_variant_index(i as i32);
        self.view
            .set_preview(strings(order.iter().enumerate().map(|(index, &id)| {
                format!(
                    "{}. {}{}",
                    index + 1,
                    s.track(id).name,
                    if s.fixed[index] { " · fixed" } else { "" }
                )
            })));
        let difficult: Vec<_> = order
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| {
                let (a, b) = (s.track(pair[0]), s.track(pair[1]));
                crate_order::difficulty(a, b).map(|reason| {
                    (
                        index,
                        format!(
                            "{}→{} · {} → {} · {reason}",
                            index + 1,
                            index + 2,
                            a.name,
                            b.name
                        ),
                    )
                })
            })
            .collect();
        self.hard = difficult.iter().map(|(index, _)| *index).collect();
        self.view
            .set_transitions(strings(difficult.into_iter().map(|(_, text)| text)));
        self.transition = (!self.hard.is_empty()).then_some(0);
        self.view
            .set_transition_index(self.transition.map_or(-1, |i| i as i32));
        self.view.set_status(if self.hard.is_empty() {
            "No difficult transitions identified.".into()
        } else {
            format!("{} difficult transitions.", self.hard.len()).into()
        });
    }
    fn pair(&self) -> Option<[i32; 2]> {
        let i = *self.hard.get(self.transition?)?;
        let order = self.variants.get(self.variant?)?;
        Some([*order.get(i)?, *order.get(i + 1)?])
    }
}

type SavedView = Option<Vec<i32>>;
struct Save {
    rx: Receiver<Result<(), String>>,
    selected: Vec<i32>,
}
#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    work: Lane<Work>,
    audio: Lane<PreviewClip>,
    owner: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
    saving: Option<Save>,
    browse_scope: Option<(u64, i32)>,
    browse: Lane<SavedView>,
}
impl State {
    fn stop(&mut self) {
        self.audio.cancel();
        if let Some((owner, epoch)) = self.owner.take() {
            if let Some(engine) = owner.upgrade().filter(|e| e.preview_epoch() == epoch) {
                engine.stop_tempo_preview();
            }
        }
    }
    fn dismiss(&mut self) {
        self.work.cancel();
        self.stop();
        if let Some(s) = self.session.take() {
            let _ = s.view.hide();
        }
    }
}

impl AudioController {
    pub(super) fn wire_crate_arrangement(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_crate_order(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_crate_arrangement() {
                    if let Some(ui) = c.ui.upgrade() {
                        ui.set_status(e.into());
                    }
                }
            }
        });
    }
    fn open_crate_arrangement(self: &Rc<Self>) -> Result<(), String> {
        if let Some(s) = &self.arrangement.borrow().session {
            return s.view.show().map_err(|e| e.to_string());
        }
        let v = CrateOrderWindow::new().map_err(|e| e.to_string())?;
        macro_rules! wire {
            ($callback:ident, $method:ident $(, $arg:ident)*) => {{
                let weak = Rc::downgrade(self);
                v.$callback(move |$($arg),*| {
                    if let Some(c) = weak.upgrade() {
                        if let Err(e) = c.$method($($arg),*) {
                            if let Some(s) = &c.arrangement.borrow().session { s.view.set_status(e.into()); }
                        }
                    }
                });
            }};
        }
        wire!(on_reload_order, reload_order);
        wire!(on_toggle_anchor, toggle_order_anchor, i);
        wire!(on_generate_order, generate_order, seed);
        wire!(on_choose_variant, choose_order_variant, i);
        wire!(on_choose_transition, choose_order_transition, i);
        wire!(on_audition, audition_order);
        wire!(on_apply_order, apply_order);
        let weak = Rc::downgrade(self);
        v.on_stop_audition(move || {
            if let Some(c) = weak.upgrade() {
                c.arrangement.borrow_mut().stop();
            }
        });
        let weak = Rc::downgrade(self);
        v.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.dismiss_order();
            }
        });
        let weak = Rc::downgrade(self);
        v.window().on_close_requested(move || {
            if weak.upgrade().is_some_and(|c| !c.dismiss_order()) {
                slint::CloseRequestResponse::KeepWindowShown
            } else {
                slint::CloseRequestResponse::HideWindow
            }
        });
        v.show().map_err(|e| e.to_string())?;
        self.arrangement.borrow_mut().session = Some(Session {
            view: v,
            generation: self.library_generation.get(),
            crate_id: 0,
            revision: 0,
            source: None,
            variants: Vec::new(),
            variant: None,
            hard: Vec::new(),
            transition: None,
        });
        if let Err(error) = self.reload_order() {
            if let Some(s) = &self.arrangement.borrow().session {
                s.view.set_status(error.into());
            }
        }
        Ok(())
    }
    fn dismiss_order(&self) -> bool {
        let mut state = self.arrangement.borrow_mut();
        if state.saving.is_some() {
            return false;
        }
        state.dismiss();
        true
    }
    pub(super) fn close_crate_arrangement(&self) {
        let mut st = self.arrangement.borrow_mut();
        st.dismiss();
        st.saving = None; // Already submitted writes stay ordered before store shutdown.
        st.browse.cancel();
        st.browse_scope = None;
    }
    fn reload_order(&self) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        if st.saving.is_some() {
            return Err("Wait for the pending save.".into());
        }
        st.stop();
        st.work.cancel();
        let s = st.session.as_mut().ok_or("Setlist window closed.")?;
        s.clear_variants();
        s.source = None;
        s.view.set_anchors(Default::default());
        s.view.set_busy(false);
        let lib = self.library.borrow();
        let id = lib.active_crate();
        s.crate_id = id;
        s.revision = lib.revision();
        let c = lib
            .crates()
            .entries
            .iter()
            .find(|c| c.id == id && c.smart_query.is_none())
            .ok_or("Select an ordinary crate in the library, then reload.")?;
        s.view.set_crate_label(c.name.clone().into());
        if c.tracks.len() > crate_order::MAX_ENTRIES {
            return Err(format!(
                "Choose a crate with at most {} entries.",
                crate_order::MAX_ENTRIES
            ));
        }
        if !self.storage_ready.get() || self.saving.get() || lib.has_changes() {
            return Err(
                "Library changes are being saved. Reload the crate when saving finishes.".into(),
            );
        }
        let handle = self
            .persistence
            .borrow()
            .as_ref()
            .ok_or("Library storage unavailable.")?
            .crate_orders();
        s.view.set_busy(true);
        s.view.set_status("Reading crate order…".into());
        st.work.submit(Box::new(move |c| {
            c.check()?;
            handle.crate_order(id).map(Work::Loaded)
        }));
        Ok(())
    }
    fn toggle_order_anchor(&self, index: i32) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        if st.saving.is_some() {
            return Ok(());
        }
        st.stop();
        st.work.cancel();
        let s = st.session.as_mut().ok_or("Setlist window closed.")?;
        let source = s.source.as_mut().ok_or("Reload the crate first.")?;
        let fixed = source
            .fixed
            .get_mut(index as usize)
            .ok_or("Choose an existing position.")?;
        *fixed = !*fixed;
        source.show_anchors(&s.view);
        s.clear_variants();
        s.view.set_busy(false);
        s.view
            .set_status("Anchor positions changed. Generate new variants.".into());
        Ok(())
    }
    fn generate_order(&self, seed: slint::SharedString) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        if st.saving.is_some() {
            return Ok(());
        }
        st.stop();
        st.work.cancel();
        let s = st.session.as_mut().ok_or("Setlist window closed.")?;
        s.clear_variants();
        s.view.set_busy(false);
        let seed = seed
            .trim()
            .parse::<u64>()
            .map_err(|_| "Enter a non-negative whole-number seed.")?;
        let source = s.source.as_ref().ok_or("Reload the crate first.")?;
        if !same_members(&self.library.borrow(), s.crate_id, &source.ids) {
            return Err("Crate membership changed. Reload the crate.".into());
        }
        let tracks = Arc::clone(&source.tracks);
        let fixed = source.fixed.clone();
        s.view.set_busy(true);
        s.view.set_status("Generating order variants…".into());
        st.work.submit(Box::new(move |c| {
            c.check()?;
            crate_order::variants(&tracks, &fixed, seed).map(Work::Variants)
        }));
        Ok(())
    }
    fn choose_order_variant(&self, i: i32) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        if st.saving.is_some() {
            return Ok(());
        }
        st.stop();
        if let Some(s) = &mut st.session {
            s.choose(i as usize);
        }
        Ok(())
    }
    fn choose_order_transition(&self, i: i32) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        if st.saving.is_some() {
            return Ok(());
        }
        st.stop();
        if let Some(s) = &mut st.session {
            s.transition = (i >= 0 && (i as usize) < s.hard.len()).then_some(i as usize);
            s.view
                .set_transition_index(s.transition.map_or(-1, |i| i as i32));
        }
        Ok(())
    }
    fn apply_order(&self) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        if st.saving.is_some() {
            return Ok(());
        }
        st.stop();
        let s = st.session.as_ref().ok_or("Setlist window closed.")?;
        let source = s.source.as_ref().ok_or("Reload the crate first.")?;
        let selected = s
            .variant
            .and_then(|i| s.variants.get(i))
            .ok_or("Choose an order variant.")?;
        let lib = self.library.borrow();
        if s.generation != self.library_generation.get()
            || !self.storage_ready.get()
            || !same_members(&lib, s.crate_id, &source.ids)
        {
            return Err("Crate membership changed. Reload and generate a fresh preview.".into());
        }
        if lib.has_changes() || self.saving.get() {
            return Err(
                "Library changes are being saved. Apply again when saving finishes.".into(),
            );
        }
        let handle = self
            .persistence
            .borrow()
            .as_ref()
            .ok_or("Library storage unavailable.")?
            .crate_orders();
        let rx = handle.begin_replace_crate_order(s.crate_id, &source.ids, selected)?;
        s.view.set_busy(true);
        s.view.set_saving(true);
        s.view.set_status("Saving the chosen order…".into());
        st.saving = Some(Save {
            rx,
            selected: selected.clone(),
        });
        Ok(())
    }

    fn audition_order(&self) -> Result<(), String> {
        let mut st = self.arrangement.borrow_mut();
        st.stop();
        let s = st.session.as_ref().ok_or("Setlist window closed.")?;
        let [a, b] = s.pair().ok_or("Choose a difficult transition.")?;
        let lib = self.library.borrow();
        if !s
            .source
            .as_ref()
            .is_some_and(|source| same_members(&lib, s.crate_id, &source.ids))
        {
            return Err("Crate membership changed. Reload before auditioning.".into());
        }
        let get = |id| {
            lib.planning_track(id)
                .filter(|t| t.available)
                .ok_or("A transition source is unavailable.")
        };
        let (outgoing, incoming) = (get(a)?, get(b)?);
        let len = |id| lib.track_by_id(id).and_then(|t| t.metadata.duration_ms);
        let millis = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        let length_ms = 6000.min(len(a).unwrap_or(6000)).min(len(b).unwrap_or(6000));
        let start = |cue: Option<Duration>, end: Option<u64>, fallback: u64| {
            let position = cue.map_or(fallback, millis);
            end.map_or(position, |d| position.min(d.saturating_sub(length_ms)))
        };
        let timing = PairTiming {
            starts_ms: [
                start(
                    outgoing.exit,
                    len(a),
                    len(a).unwrap_or(length_ms).saturating_sub(length_ms),
                ),
                start(incoming.entry, len(b), 0),
            ],
            length_ms,
            offset_b_ms: 0,
            tempos: [1.; 2],
            pitches: [0; 2],
        };
        timing.validate()?;
        let engine = self.engine.borrow();
        if !engine.preview_route_available() || self.pending_device_switch.borrow().is_some() {
            return Err("Select a separate headphone output first.".into());
        }
        s.view.set_status(
            format!(
                "Preparing {} at {:.1}s → {} at {:.1}s.",
                outgoing.name,
                timing.starts_ms[0] as f64 / 1000.,
                incoming.name,
                timing.starts_ms[1] as f64 / 1000.
            )
            .into(),
        );
        engine.stop_tempo_preview();
        st.owner = Some((Rc::downgrade(&engine), engine.preview_epoch()));
        st.audio.submit(Box::new(move |cancel| {
            use super::local_keys::Version;
            let paths = [outgoing.path, incoming.path];
            let a = Version::read(&paths[0], cancel)?;
            let b = Version::read(&paths[1], cancel)?;
            let clip =
                crate::playback::chord_preview::render(&paths[0], &paths[1], timing, cancel)?;
            if a != Version::read(&paths[0], cancel)? || b != Version::read(&paths[1], cancel)? {
                return Err("A transition source changed during preview.".into());
            }
            Ok(clip)
        }));
        Ok(())
    }

    fn poll_saved_order_view(&self) {
        let mut st = self.arrangement.borrow_mut();
        let id = self.library.borrow().active_crate();
        let scope = (self.library_generation.get(), id);
        if st.browse_scope != Some(scope) {
            st.browse.cancel();
            st.browse_scope = None;
            if !self.storage_ready.get() {
                return;
            }
            // A newly created crate may not have reached SQLite yet.
            if id > 0 && (self.saving.get() || self.library.borrow().has_changes()) {
                return;
            }
            st.browse_scope = Some(scope);
            if id > 0
                && self
                    .library
                    .borrow()
                    .crates()
                    .entries
                    .iter()
                    .any(|c| c.id == id && c.smart_query.is_none())
            {
                if let Some(store) = self.persistence.borrow().as_ref() {
                    let handle = store.crate_orders();
                    st.browse.submit(Box::new(move |c| {
                        c.check()?;
                        handle.saved_crate_order(id)
                    }));
                }
            }
        }
        let Some(result) = st.browse.poll() else {
            return;
        };
        drop(st);
        match result {
            Ok(order) => {
                let changed = self
                    .library
                    .borrow_mut()
                    .set_saved_order(id, order.as_deref());
                if changed {
                    self.publish_library();
                }
            }
            Err(e) => {
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_status(format!("Cannot read saved setlist: {e}").into());
                }
            }
        }
    }

    pub(super) fn poll_crate_arrangement(&self) {
        self.poll_saved_order_view();
        let mut st = self.arrangement.borrow_mut();
        let Some(s) = &st.session else {
            return;
        };
        if s.generation != self.library_generation.get() || !self.storage_ready.get() {
            st.dismiss();
            st.saving = None;
            return;
        }
        if let Some(save) = &st.saving {
            let result = match save.rx.try_recv() {
                Ok(v) => v,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("Storage stopped before replying.".into()),
            };
            let id = s.crate_id;
            let save = st.saving.take().expect("pending save");
            match result {
                Ok(()) => {
                    st.browse.cancel();
                    st.browse_scope = None;
                    st.dismiss();
                    drop(st);
                    self.library
                        .borrow_mut()
                        .set_saved_order(id, Some(&save.selected));
                    self.publish_library();
                    if let Some(ui) = self.ui.upgrade() {
                        ui.set_status("Saved the new crate setlist order.".into());
                    }
                }
                Err(e) => {
                    if let Some(s) = &mut st.session {
                        s.view.set_busy(false);
                        s.view.set_saving(false);
                        s.clear_variants();
                        s.view.set_status(
                            format!(
                                "Could not apply the order: {e} Reload before generating again."
                            )
                            .into(),
                        );
                        s.source = None;
                        s.view.set_anchors(Default::default());
                    }
                }
            }
            return;
        }
        let lib = self.library.borrow();
        let id = lib.active_crate();
        let revision = lib.revision();
        let changed = s.crate_id != id
            || (s.revision != revision
                && s.source.as_ref().is_some_and(|source| {
                    !same_members(&lib, s.crate_id, &source.ids)
                        || source.tracks.iter().any(|old| {
                            lib.planning_track(old.id).is_none_or(|t| {
                                t.bpm != old.bpm || t.key != old.key || t.name != old.name
                            })
                        })
                }));
        if changed {
            st.stop();
            st.work.cancel();
            if let Some(s) = &mut st.session {
                s.crate_id = id;
                s.revision = revision;
                s.source = None;
                s.clear_variants();
                s.view.set_busy(false);
                s.view.set_anchors(Default::default());
                s.view.set_crate_label("".into());
                s.view.set_status(
                    "The selected crate or its tracks changed. Reload to continue.".into(),
                );
            }
            return;
        }
        if let Some(s) = &mut st.session {
            s.revision = revision;
        }
        if let Some(result) = st.work.poll() {
            let s = st.session.as_mut().expect("open window");
            s.view.set_busy(false);
            let result=result.and_then(|r|{
                match r{
                    Work::Loaded(ids)=>{
                        let source=Source::from_library(&lib,s.crate_id,ids)?;
                        source.show_anchors(&s.view);
                        s.view.set_status("Check positions to keep, then generate variants.".into());
                        s.source=Some(source);
                    }
                    Work::Variants(orders)=>{
                        s.variants=orders;
                        s.view.set_variants(strings((1..=s.variants.len()).map(|i|format!("Variant {i}"))));
                        if s.variants.is_empty() {s.view.set_status("No alternative order. Leave at least two distinct entries movable.".into());}
                        else{s.choose(0);}
                    }
                }
                Ok(())
            });
            if let Err(e) = result {
                s.view.set_status(e.into());
            }
        }
        let engine = self.engine.borrow();
        let route =
            engine.preview_route_available() && self.pending_device_switch.borrow().is_none();
        if st.owner.as_ref().is_some_and(|(weak, epoch)| {
            !route
                || weak
                    .upgrade()
                    .is_none_or(|e| !Rc::ptr_eq(&e, &engine) || e.preview_epoch() != *epoch)
        }) {
            st.stop();
            if let Some(s) = &st.session {
                s.view.set_status("Headphone preview interrupted.".into());
            }
        }
        let audio = st.audio.poll();
        if let Some(output) = audio {
            if st.owner.is_some() {
                let result = output.and_then(|clip| engine.play_sample_preview(&clip));
                match result {
                    Ok(()) => {
                        st.owner = Some((Rc::downgrade(&engine), engine.preview_epoch()));
                        if let Some(s) = &st.session {
                            s.view.set_status(
                                "Playing the selected transition in headphones.".into(),
                            );
                        }
                    }
                    Err(e) => {
                        st.owner = None;
                        if let Some(s) = &st.session {
                            s.view.set_status(e.into());
                        }
                    }
                }
            }
        }
        if let Some(s) = &st.session {
            s.view.set_has_route(route);
            s.view.set_can_audition(
                route
                    && s.pair().is_some_and(|ids| {
                        ids.iter()
                            .all(|&id| lib.track_by_id(id).is_some_and(|t| t.available))
                    }),
            );
        }
    }
}
