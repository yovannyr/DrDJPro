use super::*;
use crate::chord_timing::PairTiming;
use crate::controller::local_keys::Version;
use crate::playback::PreviewClip;
use crate::set_planner::{Bridge, Leg};

struct Hit {
    bridge: Bridge,
    version: Option<Arc<Version>>,
}
struct Results {
    pins: [Option<Arc<Version>>; 2],
    hits: Vec<Hit>,
}
#[derive(Default)]
pub(super) struct BridgeState {
    search: Lane<Results>,
    audio: Lane<PreviewClip>,
    found: Option<Results>,
    selected: Option<usize>,
    monitor: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
}
impl BridgeState {
    fn stop(&mut self) {
        self.audio.cancel();
        if let Some((device, epoch)) = self.monitor.take() {
            if let Some(e) = device.upgrade().filter(|e| e.preview_epoch() == epoch) {
                e.stop_tempo_preview();
            }
        }
    }
    pub(super) fn clear(&mut self) {
        self.stop();
        self.search.cancel();
        self.found = None;
        self.selected = None;
    }
}
impl State {
    pub(super) fn clear_bridge_results(&mut self, notice: &str) {
        self.bridge.clear();
        if let Some(s) = &self.session {
            s.view.set_bridge_rows(Default::default());
            s.view.set_bridge_index(-1);
            s.view.set_bridge_can_audition_ab(false);
            s.view.set_bridge_can_audition_bc(false);
            s.view.set_bridge_status(notice.into());
        }
    }
}
fn describe(label: &str, leg: &Leg, readable: bool) -> String {
    let hearing = match (leg.from_exit, leg.to_entry, readable) {
        (Some(a), Some(b), true) => {
            format!("out {:.3} s → in {:.3} s", a.as_secs_f64(), b.as_secs_f64())
        }
        (Some(_), Some(_), false) => "Audio source unavailable".into(),
        _ => "Missing exit or entry marker".into(),
    };
    format!("{label} · {} / 100 · {} · {hearing}", leg.score, leg.reason)
}
fn timing(leg: &Leg, shift: Option<i8>, second: bool) -> Result<PairTiming, String> {
    let millis = |t: Option<Duration>| {
        t.and_then(|t| u64::try_from(t.as_millis()).ok())
            .ok_or("This leg lacks an exit or entry marker.")
    };
    let shift = i32::from(shift.unwrap_or(0));
    let timing = PairTiming {
        starts_ms: [millis(leg.from_exit)?, millis(leg.to_entry)?],
        length_ms: 3000,
        offset_b_ms: 0,
        tempos: [1., 1.],
        pitches: if second { [shift, 0] } else { [0, shift] },
    };
    timing.validate()?;
    Ok(timing)
}
fn stamp(path: &Path, c: &Cancellation) -> Option<Arc<Version>> {
    Version::read(path, c).ok().map(Arc::new)
}
fn render_sources(
    paths: [PathBuf; 2],
    versions: [Arc<Version>; 2],
    timing: PairTiming,
    c: &Cancellation,
) -> Result<PreviewClip, String> {
    let verify = || -> Result<(), String> {
        for (path, version) in paths.iter().zip(&versions) {
            c.check()?;
            if Version::read(path, c)? != **version {
                return Err("A source changed. Search again before auditioning this leg.".into());
            }
        }
        Ok(())
    };
    verify()?;
    let clip = crate::playback::chord_preview::render(&paths[0], &paths[1], timing, c)?;
    verify()?;
    Ok(clip)
}
fn audition_ready(found: &Results, idx: usize, second: bool) -> bool {
    let Some(h) = found.hits.get(idx) else {
        return false;
    };
    let leg = if second {
        &h.bridge.outgoing
    } else {
        &h.bridge.incoming
    };
    h.version.is_some()
        && found.pins[usize::from(second)].is_some()
        && leg.from_exit.is_some()
        && leg.to_entry.is_some()
}
impl AudioController {
    pub(super) fn wire_bridges(self: &Rc<Self>, view: &SetPlanningWindow) {
        let weak = Rc::downgrade(self);
        view.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_set_planning();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_set_destination(move || {
            if let Some(c) = weak.upgrade() {
                c.pin_destination();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_search_bridges(move |use_bpm, bpm, use_key, key| {
            if let Some(c) = weak.upgrade() {
                c.search_bridges(use_bpm, &bpm, use_key, key);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_invalidate_bridges(move || {
            if let Some(c) = weak.upgrade() {
                c.set_planning
                    .borrow_mut()
                    .clear_bridge_results("Criteria changed. Search again.");
            }
        });
        let weak = Rc::downgrade(self);
        view.on_select_bridge(move |i| {
            if let Some(c) = weak.upgrade() {
                let mut state = c.set_planning.borrow_mut();
                state.bridge.stop();
                state.bridge.selected = usize::try_from(i).ok().filter(|i| {
                    state
                        .bridge
                        .found
                        .as_ref()
                        .is_some_and(|f| *i < f.hits.len())
                });
                drop(state);
                c.poll_set_planning();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_audition_bridge(move |second| {
            if let Some(c) = weak.upgrade() {
                c.audition_bridge_leg(second);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_stop_bridge(move || {
            if let Some(c) = weak.upgrade() {
                c.set_planning.borrow_mut().bridge.stop();
            }
        });
    }
    fn pin_destination(&self) {
        let t = self.selected_planning_track();
        let mut state = self.set_planning.borrow_mut();
        state.clear_bridge_results("Select the search criteria.");
        let Some(s) = &mut state.session else {
            return;
        };
        match t {
            Ok(t) if s.anchor.as_ref().is_some_and(|a| a.id == t.id) => s
                .view
                .set_bridge_status("Choose a different destination track.".into()),
            Ok(t) => {
                s.view.set_destination_label(pin_label(&t));
                s.destination = Some(t);
            }
            Err(e) => s.view.set_bridge_status(e.into()),
        }
    }
    fn search_bridges(&self, bpm_enabled: bool, bpm: &str, key_enabled: bool, limit: i32) {
        self.poll_set_planning();
        let mut state = self.set_planning.borrow_mut();
        state.clear_bridge_results("");
        let Some(s) = &state.session else {
            return;
        };
        let view = &s.view;
        let (Some(a), Some(z)) = (&s.anchor, &s.destination) else {
            view.set_bridge_status("Choose both an anchor and a destination.".into());
            return;
        };
        if a.id == z.id {
            view.set_bridge_status("Choose two different endpoints.".into());
            return;
        }
        let max_bpm = if bpm_enabled {
            match bpm
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.)
            {
                Some(v) => Some(v),
                None => {
                    view.set_bridge_status("Enter a non-negative BPM difference.".into());
                    return;
                }
            }
        } else {
            None
        };
        if key_enabled && !(0..=6).contains(&limit) {
            view.set_bridge_status("Choose a maximum transposition from 0 to 6.".into());
            return;
        }
        let max_key = key_enabled.then_some(limit as u8);
        let ids = [a.id, z.id];
        let paths = [a.path.clone(), z.path.clone()];
        let catalog = self.library.borrow().planning_catalog();
        view.set_bridge_status("Evaluating both transitions…".into());
        state.bridge.search.submit(Box::new(move |c| {
            let bridges = catalog.bridges(ids[0], ids[1], max_bpm, max_key, c)?;
            let pins = if bridges.is_empty() {
                [None, None]
            } else {
                paths.each_ref().map(|p| stamp(p, c))
            };
            let mut hits = Vec::with_capacity(bridges.len());
            for bridge in bridges {
                c.check()?;
                let version = stamp(&bridge.track.path, c);
                hits.push(Hit { bridge, version });
            }
            c.check()?;
            Ok(Results { pins, hits })
        }));
    }
    fn audition_bridge_leg(&self, second: bool) {
        self.poll_set_planning();
        let mut state = self.set_planning.borrow_mut();
        state.bridge.stop();
        let Some(s) = &state.session else {
            return;
        };
        let v = &s.view;
        let (Some(found), Some(i)) = (&state.bridge.found, state.bridge.selected) else {
            return;
        };
        if !audition_ready(found, i, second) {
            v.set_bridge_status("A source or listening marker is unavailable.".into());
            return;
        }
        let engine = self.engine.borrow();
        if !engine.preview_route_available() || self.pending_device_switch.borrow().is_some() {
            v.set_bridge_status("Select a separate headphone output first.".into());
            return;
        }
        let Some(pin) = (if second {
            s.destination.as_ref()
        } else {
            s.anchor.as_ref()
        }) else {
            return;
        };
        let hit = &found.hits[i];
        let bridge = &hit.bridge;
        let (Some(b_version), Some(pin_version)) = (&hit.version, &found.pins[usize::from(second)])
        else {
            return;
        };
        let (paths, versions, leg) = if second {
            (
                [bridge.track.path.clone(), pin.path.clone()],
                [Arc::clone(b_version), Arc::clone(pin_version)],
                &bridge.outgoing,
            )
        } else {
            (
                [pin.path.clone(), bridge.track.path.clone()],
                [Arc::clone(pin_version), Arc::clone(b_version)],
                &bridge.incoming,
            )
        };
        let timing = match timing(leg, bridge.shift, second) {
            Ok(v) => v,
            Err(e) => {
                v.set_bridge_status(e.into());
                return;
            }
        };
        v.set_bridge_status("Preparing the selected headphone transition…".into());
        engine.stop_tempo_preview();
        state.bridge.monitor = Some((Rc::downgrade(&engine), engine.preview_epoch()));
        state.bridge.audio.submit(Box::new(move |c| {
            render_sources(paths, versions, timing, c)
        }));
    }
    pub(super) fn poll_bridges(&self, state: &mut State) {
        let found = state.bridge.search.poll();
        let audio = state.bridge.audio.poll();
        let Some(s) = &state.session else {
            return;
        };
        let view = &s.view;
        let engine = self.engine.borrow();
        let route =
            engine.preview_route_available() && self.pending_device_switch.borrow().is_none();
        view.set_bridge_route(route);
        if state.bridge.monitor.as_ref().is_some_and(|(weak, epoch)| {
            !route
                || weak
                    .upgrade()
                    .is_none_or(|old| !Rc::ptr_eq(&old, &engine) || old.preview_epoch() != *epoch)
        }) {
            state.bridge.stop();
            view.set_bridge_status("Headphone output changed or another preview took over.".into());
        }
        match found {
            Some(Ok(f)) => {
                view.set_bridge_rows(row_model(f.hits.iter().map(|h| {
                    let shift = h.bridge.shift.map_or_else(
                        || "No evidenced transposition".into(),
                        |n| format!("{n:+} semitones"),
                    );
                    format!(
                        "{} · {}\n{}\n{}",
                        h.bridge.track.name,
                        shift,
                        describe(
                            "A → B",
                            &h.bridge.incoming,
                            f.pins[0].is_some() && h.version.is_some()
                        ),
                        describe(
                            "B → C",
                            &h.bridge.outgoing,
                            f.pins[1].is_some() && h.version.is_some()
                        )
                    )
                })));
                state.bridge.selected = (!f.hits.is_empty()).then_some(0);
                view.set_bridge_status(if f.hits.is_empty() {
                    "No bridge supported by both transitions under these criteria.".into()
                } else {
                    format!("{} bridges.", f.hits.len()).into()
                });
                state.bridge.found = Some(f);
            }
            Some(Err(e)) => view.set_bridge_status(e.into()),
            None => (),
        }
        if let Some(output) = audio {
            if let Some((_, epoch)) = &mut state.bridge.monitor {
                match output.and_then(|clip| engine.play_sample_preview(&clip)) {
                    Ok(()) => {
                        *epoch = engine.preview_epoch();
                        view.set_bridge_status("Playing marked transition in headphones.".into());
                    }
                    Err(e) => {
                        state.bridge.monitor = None;
                        view.set_bridge_status(e.into());
                    }
                }
            }
        }
        view.set_bridge_index(state.bridge.selected.map_or(-1, |i| i as i32));
        let enabled = |second| {
            route
                && state
                    .bridge
                    .selected
                    .zip(state.bridge.found.as_ref())
                    .is_some_and(|(i, f)| audition_ready(f, i, second))
        };
        view.set_bridge_can_audition_ab(enabled(false));
        view.set_bridge_can_audition_bc(enabled(true));
    }
}
