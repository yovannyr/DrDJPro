//! Modeless set preparation. The current library selection can move independently of the pins.
use super::*;
use crate::cancellation::Cancellation;
use crate::set_planner::{PlanningTrack, Variant};
use crate::SetPlanningWindow;
mod bridges;

pub(super) type Task<T> = Box<dyn FnOnce(&Cancellation) -> Result<T, String> + Send>;

/// One running and one replaceable request keep repeated clicks bounded.
pub(super) struct Lane<T> {
    next: Option<Task<T>>,
    running: Option<(Cancellation, Receiver<Result<T, String>>)>,
}
impl<T> Default for Lane<T> {
    fn default() -> Self {
        Self {
            next: None,
            running: None,
        }
    }
}
impl<T: Send + 'static> Lane<T> {
    pub(super) fn cancel(&mut self) {
        self.next = None;
        if let Some((c, _)) = &self.running {
            c.request();
        }
    }
    pub(super) fn submit(&mut self, work: Task<T>) {
        self.cancel();
        self.next = Some(work);
    }
    pub(super) fn poll(&mut self) -> Option<Result<T, String>> {
        if let Some((c, rx)) = &self.running {
            let result = match rx.try_recv() {
                Ok(v) => v,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err("Set planning worker stopped.".into()),
            };
            let cancelled = c.is_cancelled();
            self.running = None;
            if !cancelled {
                return Some(result);
            }
        }
        let work = self.next.take()?;
        let (tx, rx) = mpsc::sync_channel(1);
        let cancel = Cancellation::default();
        let token = cancel.clone();
        match std::thread::Builder::new()
            .name("set-planning".into())
            .spawn(move || {
                let _ = tx.send(work(&token));
            }) {
            Ok(_) => {
                self.running = Some((cancel, rx));
                None
            }
            Err(e) => Some(Err(e.to_string())),
        }
    }
}
impl<T> Drop for Lane<T> {
    fn drop(&mut self) {
        if let Some((token, _)) = &self.running {
            token.request();
        }
    }
}

struct Session {
    view: SetPlanningWindow,
    generation: u64,
    revision: u64,
    crate_id: i32,
    anchor: Option<PlanningTrack>,
    destination: Option<PlanningTrack>,
}

#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    variants: Lane<Vec<Variant>>,
    bridge: bridges::BridgeState,
}

fn parameters(limit: i32, seed: &str) -> Result<(u8, u64), String> {
    if !(0..=6).contains(&limit) {
        return Err("Choose 0 to 6 semitones.".into());
    }
    let text = seed.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Enter a non-negative whole-number seed.".into());
    }
    Ok((
        limit as u8,
        text.parse::<u64>()
            .map_err(|_| "Seed exceeds the unsigned 64-bit range.")?,
    ))
}
fn pin_label(track: &PlanningTrack) -> slint::SharedString {
    format!(
        "{} · {}",
        track.name,
        track
            .key
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "Unknown key".into())
    )
    .into()
}
fn row_model(rows: impl Iterator<Item = String>) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(rows.map(Into::into).collect::<Vec<_>>()))
}
impl AudioController {
    pub(super) fn wire_set_planning(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_set_planning(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_set_planning() {
                    if let Some(ui) = c.ui.upgrade() {
                        ui.set_status(e.into());
                    }
                }
            }
        });
    }
    fn open_set_planning(self: &Rc<Self>) -> Result<(), String> {
        if let Some(s) = &self.set_planning.borrow().session {
            return s.view.show().map_err(|e| e.to_string());
        }
        let view = SetPlanningWindow::new().map_err(|e| e.to_string())?;
        let weak = Rc::downgrade(self);
        view.on_open_midi_scene(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_midi_scene() {
                    if let Some(s) = &c.set_planning.borrow().session {
                        s.view.set_status(e.into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_open_package(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_package() {
                    if let Some(ui) = c.ui.upgrade() {
                        ui.set_status(e.into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_open_passages(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_passages() {
                    if let Some(s) = &c.set_planning.borrow().session {
                        s.view.set_status(e.into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_open_hook_marks(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(error) = c.open_hook_marks() {
                    if let Some(s) = &c.set_planning.borrow().session {
                        s.view.set_status(error.into());
                    }
                }
            }
        });
        self.wire_bridges(&view);
        self.wire_timeline_entry(&view);
        let weak = Rc::downgrade(self);
        view.on_set_anchor(move || {
            if let Some(c) = weak.upgrade() {
                c.set_planning_anchor();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_search_variants(move |limit, seed| {
            if let Some(c) = weak.upgrade() {
                c.search_variants(limit, &seed);
            }
        });
        let weak = Rc::downgrade(self);
        view.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_set_planning();
            }
            slint::CloseRequestResponse::HideWindow
        });
        view.show().map_err(|e| e.to_string())?;
        let session = Session {
            view,
            generation: self.library_generation.get(),
            revision: self.library.borrow().revision(),
            crate_id: self.library.borrow().active_crate(),
            anchor: None,
            destination: None,
        };
        self.set_planning.borrow_mut().session = Some(session);
        if self.library.borrow().selected_id() > 0 {
            self.set_planning_anchor();
        }
        Ok(())
    }
    pub(super) fn close_set_planning(&self) {
        let mut state = self.set_planning.borrow_mut();
        state.variants.cancel();
        state.bridge.clear();
        if let Some(s) = state.session.take() {
            let _ = s.view.hide();
        }
    }
    fn selected_planning_track(&self) -> Result<PlanningTrack, String> {
        let ui = self.ui.upgrade().ok_or("Main window closed.")?;
        let lib = self.library.borrow();
        if !self.storage_ready.get() || lib.selected_id() != ui.get_selected_track() {
            return Err("Choose a track in the current library.".into());
        }
        lib.planning_track(lib.selected_id())
            .ok_or_else(|| "Choose an available library track.".into())
    }
    fn set_planning_anchor(&self) {
        let track = self.selected_planning_track();
        let mut state = self.set_planning.borrow_mut();
        state.variants.cancel();
        state.clear_bridge_results("Anchor changed. Search again.");
        let Some(s) = &mut state.session else {
            return;
        };
        s.view.set_rows(Default::default());
        match track {
            Ok(t) => {
                s.view.set_anchor_label(pin_label(&t));
                s.anchor = Some(t);
                s.view
                    .set_status("Choose a semitone limit and search.".into());
            }
            Err(e) => {
                s.view.set_status(e.into());
            }
        }
    }
    fn search_variants(&self, limit: i32, seed: &str) {
        self.poll_set_planning();
        let mut state = self.set_planning.borrow_mut();
        state.variants.cancel();
        let Some(s) = &state.session else {
            return;
        };
        s.view.set_rows(Default::default());
        let Some(anchor) = &s.anchor else {
            s.view.set_status("Choose an anchor track first.".into());
            return;
        };
        let (limit, seed) = match parameters(limit, seed) {
            Ok(p) => p,
            Err(e) => {
                s.view.set_status(e.into());
                return;
            }
        };
        let id = anchor.id;
        let catalog = self.library.borrow().planning_catalog();
        s.view
            .set_status("Searching transposition variants…".into());
        state
            .variants
            .submit(Box::new(move |c| catalog.variants(id, limit, seed, c)));
    }
    pub(super) fn poll_set_planning(&self) {
        let mut state = self.set_planning.borrow_mut();
        if state.session.as_ref().is_some_and(|s| {
            s.generation != self.library_generation.get() || !self.storage_ready.get()
        }) {
            drop(state);
            self.close_set_planning();
            return;
        }
        let revision = self.library.borrow().revision();
        let crate_id = self.library.borrow().active_crate();
        if state
            .session
            .as_ref()
            .is_some_and(|s| s.revision != revision || s.crate_id != crate_id)
        {
            state.variants.cancel();
            state.clear_bridge_results("The catalog or crate changed. Search again.");
            if let Some(s) = &mut state.session {
                s.revision = revision;
                s.crate_id = crate_id;
                s.destination = s.destination.take().and_then(|old| {
                    self.library
                        .borrow()
                        .planning_track(old.id)
                        .filter(|t| t.path == old.path)
                });
                s.view.set_destination_label(
                    s.destination.as_ref().map(pin_label).unwrap_or_default(),
                );
                s.anchor = s.anchor.take().and_then(|old| {
                    self.library
                        .borrow()
                        .planning_track(old.id)
                        .filter(|t| t.path == old.path)
                });
                s.view
                    .set_anchor_label(s.anchor.as_ref().map(pin_label).unwrap_or_default());
                s.view.set_rows(Default::default());
                s.view
                    .set_status("Library changed. Search again for current results.".into());
            }
        }
        self.poll_bridges(&mut state);
        let done = state.variants.poll();
        let Some(s) = &state.session else {
            return;
        };
        if let Some(result) = done {
            match result {
                Ok(v) => {
                    s.view.set_rows(row_model(v.iter().map(|v| {
                        format!(
                            "{} · Original {} · {:+} semitones · {}",
                            v.track.name,
                            v.track
                                .key
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "unknown".into()),
                            v.shift,
                            v.reason
                        )
                    })));
                    s.view.set_status(if v.is_empty() {
                        "No eligible variants within the selected limit.".into()
                    } else {
                        format!("{} variants.", v.len()).into()
                    });
                }
                Err(e) => s.view.set_status(e.into()),
            }
        }
    }
}
