//! Explicit live exercise: each pad resolves against its deck's actual loaded track.
use super::*;
use crate::HookPracticeWindow;
use std::{sync::Weak, time::Instant};

#[derive(Clone)]
struct Hook {
    id: i32,
    path: PathBuf,
    title: String,
    slot: usize,
    position: Duration,
}
struct Step {
    hook: Hook,
    target: f64,
    tolerance: f64,
}
#[derive(Clone, Copy)]
enum Outcome {
    Played(Duration),
    Skipped,
}
struct Run {
    start: Instant,
    next: usize,
    outcomes: Vec<Option<Outcome>>,
    active: bool,
}
#[derive(Default)]
pub(super) struct State {
    view: Option<HookPracticeWindow>,
    source: Vec<Hook>,
    steps: Vec<Step>,
    selected: Option<usize>,
    run: Option<Run>,
    owned: Option<(DeckId, Weak<crate::deck_render::RenderControl>)>,
    pressed: [u8; 2],
}
impl State {
    fn running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.active)
    }
    fn publish(&self, msg: &str) {
        let Some(v) = &self.view else {
            return;
        };
        v.set_selected(self.selected.map_or(-1, |i| i as i32));
        v.set_running(self.running());
        v.set_steps(ModelRc::new(VecModel::from(
            self.steps
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let result = match self
                        .run
                        .as_ref()
                        .and_then(|r| r.outcomes.get(i))
                        .copied()
                        .flatten()
                    {
                        Some(Outcome::Skipped) => " · Skipped".into(),
                        Some(Outcome::Played(at)) => {
                            let delta = at.as_secs_f64() - s.target;
                            format!(
                                " · Actual {:.2} s · {delta:+.2} s · {}",
                                at.as_secs_f64(),
                                if delta.abs() <= s.tolerance {
                                    "in window"
                                } else {
                                    "outside window"
                                }
                            )
                        }
                        None => String::new(),
                    };
                    slint::SharedString::from(format!(
                        "{} · {} / Pad {} @ {:.3} s · Target {:.2} ± {:.2} s{}",
                        i + 1,
                        s.hook.title,
                        s.hook.slot + 1,
                        s.hook.position.as_secs_f64(),
                        s.target,
                        s.tolerance,
                        result
                    ))
                })
                .collect::<Vec<_>>(),
        )));
        if let Some(s) = self.selected.and_then(|i| self.steps.get(i)) {
            v.set_target_time(s.target.to_string().into());
            v.set_tolerance(s.tolerance.to_string().into());
        }
        if !msg.is_empty() {
            v.set_status(msg.into());
        }
    }
    fn stop_owned(&mut self, engine: &DualDeckEngine) {
        if let Some((id, token)) = self.owned.take() {
            if engine
                .renderer_token(id)
                .is_some_and(|current| Weak::ptr_eq(&current, &token))
            {
                engine.pause(id);
            }
        }
    }
    fn finish(&mut self, engine: &DualDeckEngine) {
        self.stop_owned(engine);
        if let Some(run) = &mut self.run {
            if run.active {
                for item in &mut run.outcomes[run.next..] {
                    *item = Some(Outcome::Skipped);
                }
                run.active = false;
            }
        }
    }
}
impl AudioController {
    pub(super) fn open_hook_practice(self: &Rc<Self>) -> Result<(), String> {
        if let Some(v) = &self.hook_practice.borrow().view {
            return v.show().map_err(|e| e.to_string());
        }
        let v = HookPracticeWindow::new().map_err(|e| e.to_string())?;
        let w = Rc::downgrade(self);
        v.on_use_track(move || {
            if let Some(c) = w.upgrade() {
                c.practice_source();
            }
        });
        let w = Rc::downgrade(self);
        v.on_add_step(move |raw| {
            if let Some(c) = w.upgrade() {
                let mut s = c.hook_practice.borrow_mut();
                if s.running() || s.steps.len() >= 5 {
                    return;
                }
                if let Some(h) = usize::try_from(raw)
                    .ok()
                    .and_then(|i| s.source.get(i))
                    .cloned()
                {
                    let time = s.steps.last().map_or(0., |last| last.target + 30.);
                    s.run = None;
                    s.steps.push(Step {
                        hook: h,
                        target: time,
                        tolerance: 2.,
                    });
                    s.selected = Some(s.steps.len() - 1);
                    s.publish("");
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_choose(move |i| {
            if let Some(c) = w.upgrade() {
                let mut s = c.hook_practice.borrow_mut();
                s.selected = usize::try_from(i).ok().filter(|i| *i < s.steps.len());
                s.publish("");
            }
        });
        let w = Rc::downgrade(self);
        v.on_move_step(move |d| {
            if let Some(c) = w.upgrade() {
                let mut s = c.hook_practice.borrow_mut();
                if s.running() {
                    return;
                }
                let Some(i) = s.selected else {
                    return;
                };
                if let Some(j) = i
                    .checked_add_signed(d as isize)
                    .filter(|j| *j < s.steps.len())
                {
                    s.run = None;
                    let other = s.steps[j].hook.clone();
                    s.steps[j].hook = s.steps[i].hook.clone();
                    s.steps[i].hook = other;
                    s.selected = Some(j);
                    s.publish("");
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_remove_step(move || {
            if let Some(c) = w.upgrade() {
                let mut s = c.hook_practice.borrow_mut();
                if !s.running() {
                    if let Some(i) = s.selected.take() {
                        s.steps.remove(i);
                        if let Some(first) = s.steps.first_mut() {
                            first.target = 0.;
                        }
                        s.run = None;
                        s.publish("");
                    }
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_edit_time(move|target,margin|{if let Some(c)=w.upgrade(){let mut s=c.hook_practice.borrow_mut();if s.running(){return;}let Some(i)=s.selected.filter(|i|*i>0)else{return;};
            let parse=|v:&str|v.trim().parse::<f64>().ok().filter(|v|v.is_finite()&&(0.0..=3600.).contains(v));
            if let Some((t,m))=parse(&target).zip(parse(&margin)).filter(|(t,_)|*t>s.steps[i-1].target&&s.steps.get(i+1).is_none_or(|n|*t<n.target)){
                s.steps[i].target=t;s.steps[i].tolerance=m;s.run=None;s.publish("Time window updated.");
            }else{s.publish("Choose increasing target times and a non-negative tolerance, up to one hour.");}}});
        let w = Rc::downgrade(self);
        v.on_load(move |b| {
            if let Some(c) = w.upgrade() {
                let s = c.hook_practice.borrow();
                let Some(h) = s.selected.and_then(|i| s.steps.get(i)) else {
                    return;
                };
                let id = if b { DeckId::B } else { DeckId::A };
                if c.engine.borrow().snapshot(id).playing || c.pending.borrow().is_some() {
                    s.publish("Use a paused, available deck for loading.");
                    return;
                }
                let path = h.hook.path.clone();
                drop(s);
                c.start_load(id, Arc::new(move || Some(path.clone())), 0.);
            }
        });
        let w = Rc::downgrade(self);
        v.on_start(move || {
            if let Some(c) = w.upgrade() {
                c.practice_trigger(None, true, None);
            }
        });
        let w = Rc::downgrade(self);
        v.on_trigger(move || {
            if let Some(c) = w.upgrade() {
                let index = c.hook_practice.borrow().selected;
                c.practice_trigger(index, false, None);
            }
        });
        let w = Rc::downgrade(self);
        v.on_skip(move || {
            if let Some(c) = w.upgrade() {
                let mut s = c.hook_practice.borrow_mut();
                if let Some(run) = s.run.as_mut().filter(|r| r.active) {
                    run.outcomes[run.next] = Some(Outcome::Skipped);
                    run.next += 1;
                    run.active = run.next < run.outcomes.len();
                    s.selected = run.active.then_some(run.next);
                    s.publish("Hook skipped.");
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_end(move || {
            if let Some(c) = w.upgrade() {
                let mut s = c.hook_practice.borrow_mut();
                s.finish(&c.engine.borrow());
                s.publish("Run ended.");
            }
        });
        let w = Rc::downgrade(self);
        v.on_dismiss(move || {
            if let Some(c) = w.upgrade() {
                c.close_hook_practice();
            }
        });
        let w = Rc::downgrade(self);
        v.window().on_close_requested(move || {
            if let Some(c) = w.upgrade() {
                c.close_hook_practice();
            }
            slint::CloseRequestResponse::HideWindow
        });
        v.show().map_err(|e| e.to_string())?;
        self.hook_practice.borrow_mut().view = Some(v);
        self.practice_source();
        self.hook_practice.borrow().publish("Arrange 3–5 hooks. Load the first hook on a deck and start the run. Use pads 1–4 on the deck holding each track.");
        Ok(())
    }
    fn practice_source(&self) {
        let mut s = self.hook_practice.borrow_mut();
        if s.running() {
            return;
        }
        let lib = self.library.borrow();
        let id = lib.selected_id();
        s.source.clear();
        let mut title = String::new();
        if let Some(t) = lib.track_by_id(id) {
            title = if t.metadata.title.is_empty() {
                t.path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                t.metadata.title.clone()
            };
            s.source.extend(
                t.hot_cues
                    .slots()
                    .iter()
                    .take(4)
                    .enumerate()
                    .filter_map(|(i, h)| {
                        h.as_ref().map(|h| Hook {
                            id,
                            path: t.path.clone(),
                            title: title.clone(),
                            slot: i,
                            position: h.position(),
                        })
                    }),
            );
        }
        if let Some(v) = &s.view {
            v.set_track(title.into());
            v.set_cues(ModelRc::new(VecModel::from(
                s.source
                    .iter()
                    .map(|h| {
                        slint::SharedString::from(format!(
                            "Pad {} · {:.3} s",
                            h.slot + 1,
                            h.position.as_secs_f64()
                        ))
                    })
                    .collect::<Vec<_>>(),
            )));
        }
    }
    pub(super) fn close_hook_practice(&self) {
        let mut s = self.hook_practice.borrow_mut();
        s.finish(&self.engine.borrow());
        if let Some(v) = s.view.take() {
            let _ = v.hide();
        }
        s.steps.clear();
        s.source.clear();
        s.run = None;
        s.selected = None;
    }
    fn practice_trigger(&self, index: Option<usize>, start: bool, requested_deck: Option<DeckId>) {
        let mut s = self.hook_practice.borrow_mut();
        let (index, next) = if start {
            if s.running() || !(3..=5).contains(&s.steps.len()) {
                return;
            }
            (0, 0)
        } else {
            let Some(run) = s.run.as_ref().filter(|r| r.active) else {
                return;
            };
            let Some(i) = index.filter(|i| *i >= run.next && *i < s.steps.len()) else {
                s.publish("Choose a remaining hook in this run.");
                return;
            };
            (i, run.next)
        };
        let hook = &s.steps[index].hook;
        let engine = self.engine.borrow();
        let id = requested_deck.or_else(|| {
            [DeckId::A, DeckId::B]
                .into_iter()
                .find(|id| engine.snapshot(*id).path.as_deref() == Some(hook.path.as_path()))
        });
        let Some(id) = id else {
            s.publish("Load this hook's track onto a paused deck first.");
            return;
        };
        let snapshot = engine.snapshot(id);
        if !self.can_control_deck(id)
            || snapshot.path.as_deref() != Some(hook.path.as_path())
            || snapshot.loop_active
            || engine.platter_touched(id)
            || engine.censoring(id)
            || self.sync_session.borrow().is_some()
        {
            s.publish("Release loop, platter, censor and Sync on the practice deck.");
            return;
        }
        let valid = self.library.borrow().track_by_id(hook.id).is_some_and(|t| {
            t.path == hook.path
                && t.hot_cues.slots()[hook.slot]
                    .as_ref()
                    .is_some_and(|cue| cue.position() == hook.position)
        });
        if !valid || hook.position >= snapshot.duration {
            s.publish("The saved cue or source changed. End the run and choose it again.");
            return;
        }
        let now = engine.control_now();
        if let Err(e) = engine.seek_position(id, hook.position) {
            s.publish(&e);
            return;
        }
        if s.owned.as_ref().is_some_and(|(old, _)| *old != id) {
            s.stop_owned(&engine);
        }
        engine.play(id);
        if !engine.snapshot(id).playing {
            s.publish("The practice deck could not start.");
            return;
        }
        s.owned = engine.renderer_token(id).map(|token| (id, token));
        if start {
            s.run = Some(Run {
                start: now,
                next: 0,
                outcomes: vec![None; s.steps.len()],
                active: true,
            });
        }
        let Some(run) = &mut s.run else {
            return;
        };
        for item in &mut run.outcomes[next..index] {
            *item = Some(Outcome::Skipped);
        }
        run.outcomes[index] = Some(Outcome::Played(now.saturating_duration_since(run.start)));
        run.next = index + 1;
        run.active = run.next < run.outcomes.len();
        let status = if run.active {
            "Play the next hook on its loaded deck, or skip it."
        } else {
            "Run complete. End run stops the final hook."
        };
        s.selected = Some(if run.active { run.next } else { index });
        s.publish(status);
        drop(s);
        drop(engine);
        self.record_history(id);
    }
    pub(super) fn practice_pad(&self, deck: usize, slot: usize, held: bool, delete: bool) -> bool {
        let mut s = self.hook_practice.borrow_mut();
        let mask = 1u8 << slot;
        if !held && s.pressed[deck] & mask != 0 {
            s.pressed[deck] &= !mask;
            return true;
        }
        if !s.running() {
            return false;
        }
        if !held {
            return false;
        }
        if s.pressed[deck] & mask != 0 {
            return true;
        }
        s.pressed[deck] |= mask;
        if delete {
            return true;
        }
        let id = if deck == 0 { DeckId::A } else { DeckId::B };
        let path = self.engine.borrow().snapshot(id).path;
        let next = s.run.as_ref().map_or(0, |r| r.next);
        let index = s
            .steps
            .iter()
            .enumerate()
            .skip(next)
            .find(|(_, h)| h.hook.slot == slot && Some(h.hook.path.as_path()) == path.as_deref())
            .map(|(i, _)| i);
        if index.is_none() {
            s.publish("This deck and pad do not match a remaining hook.");
            return true;
        }
        drop(s);
        self.practice_trigger(index, false, Some(id));
        true
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    use crate::{hot_cues::HotCue, test_support::wav_samples};
    use std::cell::Cell;

    #[test]
    fn d09_two_tracks_reuse_a_pad_and_report_actual_or_skipped_times() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let origin = Instant::now();
        let clock = Rc::new(Cell::new(origin));
        let read_clock = clock.clone();
        let (mixer, _out) = rodio::mixer::mixer(4, 48000);
        let engine = Rc::new(DualDeckEngine::from_mixer_with_clock(
            mixer,
            4,
            48000,
            Box::new(move || read_clock.get()),
        ));
        let c = AudioController::wire(&ui, engine.clone(), Arc::new(|| None), Arc::new(|| None));
        c.storage_ready.set(true);
        let a = wav_samples(&[2048; 4000], 8000, 1);
        let b = wav_samples(&[-2048; 4000], 8000, 1);
        let a_path = a.path().canonicalize().unwrap();
        let b_path = b.path().canonicalize().unwrap();
        c.library
            .borrow_mut()
            .insert([a_path.clone(), b_path.clone()]);
        let positions = [100, 150, 200].map(Duration::from_millis);
        for ((path, slot), position) in [(&a_path, 0), (&b_path, 0), (&a_path, 1)]
            .into_iter()
            .zip(positions)
        {
            c.library
                .borrow_mut()
                .set_hot_cue(
                    path,
                    slot,
                    Some(HotCue::new(position, "Cue".into(), 0x336699).unwrap()),
                )
                .unwrap();
            let index = c.hook_practice.borrow().steps.len();
            let id = c.library.borrow().track_id_for_path(path).unwrap();
            c.hook_practice.borrow_mut().steps.push(Step {
                hook: Hook {
                    id,
                    path: path.clone(),
                    title: format!("track {id}"),
                    slot,
                    position,
                },
                target: (index * 30) as f64,
                tolerance: 1.0,
            });
        }
        engine.load_a(&a_path).unwrap();
        engine.load_b(&b_path).unwrap();
        c.practice_trigger(None, true, None);
        let hit = |deck, slot| {
            assert!(c.practice_pad(deck, slot, true, false));
            assert!(c.practice_pad(deck, slot, false, false));
        };
        clock.set(origin + Duration::from_secs(31));
        hit(1, 0);
        clock.set(origin + Duration::from_secs(58));
        hit(0, 1);
        {
            let state = c.hook_practice.borrow();
            let run = state.run.as_ref().unwrap();
            assert!(!run.active);
            let actual = run
                .outcomes
                .iter()
                .map(|v| match v {
                    Some(Outcome::Played(t)) => Some(t.as_secs()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, [Some(0), Some(31), Some(58)]);
        }
        c.practice_trigger(None, true, None);
        clock.set(origin + Duration::from_secs(118));
        hit(0, 1);
        {
            let state = c.hook_practice.borrow();
            let run = state.run.as_ref().unwrap();
            assert!(matches!(run.outcomes[1], Some(Outcome::Skipped)));
            assert!(
                matches!(run.outcomes[2], Some(Outcome::Played(time)) if time==Duration::from_secs(60))
            );
        }
        c.close_hook_practice();
        for ((path, slot), pos) in [(&a_path, 0), (&b_path, 0), (&a_path, 1)]
            .into_iter()
            .zip(positions)
        {
            assert_eq!(
                c.library.borrow().hot_cues(path).slots()[slot]
                    .as_ref()
                    .unwrap()
                    .position(),
                pos
            );
        }
    }
}
