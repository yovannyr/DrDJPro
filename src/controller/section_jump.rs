//! A deck owns its armed cue or phrase until its renderer consumes or revokes that ticket.
use super::*;
use crate::beatgrid::BeatGrid;
use crate::playback::{DeckSnapshot, PendingSectionJump, SectionJumpStatus};
use std::borrow::Cow;
use std::sync::Weak;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Cue(usize),
    Phrase,
}

#[derive(PartialEq, Eq)]
struct JumpTarget {
    kind: TargetKind,
    at: Duration,
    label: slint::SharedString,
}

fn targets(track: &crate::library::Track, duration: Duration) -> Vec<JumpTarget> {
    let mut targets = Vec::new();
    for (slot, cue) in track.hot_cues.slots().iter().enumerate() {
        if let Some(cue) = cue.as_ref().filter(|c| c.position() < duration) {
            targets.push(JumpTarget {
                kind: TargetKind::Cue(slot),
                at: cue.position(),
                label: format!(
                    "Cue {} · {} · {:.3} s",
                    slot + 1,
                    cue.name(),
                    cue.position().as_secs_f64()
                )
                .into(),
            });
        }
    }
    if let Some(analysis) = &track.metadata.analysis {
        for (index, ms) in analysis.phrase_boundaries_ms.iter().enumerate() {
            let at = Duration::from_millis(u64::from(*ms));
            if at < duration {
                targets.push(JumpTarget {
                    kind: TargetKind::Phrase,
                    at,
                    label: format!("Phrase {} · {:.3} s", index + 1, at.as_secs_f64()).into(),
                });
            }
        }
    }
    targets
}

#[derive(Default)]
pub(super) struct SectionDeck {
    source: Weak<crate::waveform::DetailPeaks>,
    revision: Option<(u64, u64)>,
    track_id: Option<i32>,
    saved_grid: Option<BeatGrid>,
    live_grid: Option<BeatGrid>,
    targets: Vec<JumpTarget>,
    selected: Option<usize>,
    pending: Option<PendingSectionJump>,
    notice: slint::SharedString,
}

impl SectionDeck {
    fn finish(&mut self, cancelled: &str) {
        if let Some(ticket) = self.pending.take() {
            self.notice = match ticket.cancel_if_pending() {
                SectionJumpStatus::Executed => "Section jump executed.".into(),
                _ => cancelled.into(),
            };
        }
    }

    fn replace_targets(&mut self, ui: &AppWindow, id: DeckId, new: Vec<JumpTarget>) {
        if self.targets == new {
            return;
        }
        let selected = self
            .selected
            .and_then(|i| self.targets.get(i))
            .and_then(|old| {
                new.iter()
                    .position(|t| t.kind == old.kind && t.at == old.at)
            });
        if self.selected.is_some() && selected.is_none() {
            self.finish("Section jump cancelled. The target changed.");
        }
        self.selected = selected;
        let rows = ModelRc::new(VecModel::from(
            new.iter().map(|t| t.label.clone()).collect::<Vec<_>>(),
        ));
        match id {
            DeckId::A => ui.set_section_targets_left(rows),
            DeckId::B => ui.set_section_targets_right(rows),
        }
        self.targets = new;
    }
}
impl Drop for SectionDeck {
    fn drop(&mut self) {
        if let Some(ticket) = self.pending.take() {
            ticket.cancel_if_pending();
        }
    }
}

fn deck(index: i32) -> Option<DeckId> {
    match index {
        0 => Some(DeckId::A),
        1 => Some(DeckId::B),
        _ => None,
    }
}

impl AudioController {
    pub(super) fn wire_section_jumps(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_select_section_target(move |index, selected| {
            if let Some((c, id)) = weak.upgrade().zip(deck(index)) {
                c.select_section_target(id, selected);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_arm_section_jump(move |index| {
            if let Some((c, id)) = weak.upgrade().zip(deck(index)) {
                c.arm_section_jump(id);
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_cancel_section_jump(move |index| {
            if let Some((c, id)) = weak.upgrade().zip(deck(index)) {
                c.section_jumps.borrow_mut()[Self::grid_index(id)]
                    .finish("Section jump cancelled.");
                c.refresh_section_now(id);
            }
        });
    }

    fn refresh_section_now(&self, id: DeckId) {
        if let Some(ui) = self.ui.upgrade() {
            let snapshot = self.engine.borrow().snapshot(id);
            self.refresh_section_jump(&ui, id, &snapshot);
        }
    }

    // A confirmed Prepare cue edit does not need a second library save. Refresh
    // the cached target list explicitly, even when no dirty revision was made.
    pub(super) fn cue_targets_changed(&self) {
        for id in [DeckId::A, DeckId::B] {
            self.section_jumps.borrow_mut()[Self::grid_index(id)].revision = None;
            self.refresh_section_now(id);
        }
    }

    fn select_section_target(&self, id: DeckId, raw: i32) {
        self.refresh_section_now(id);
        let mut decks = self.section_jumps.borrow_mut();
        let state = &mut decks[Self::grid_index(id)];
        let next = usize::try_from(raw)
            .ok()
            .filter(|i| *i < state.targets.len());
        if next != state.selected {
            state.notice = Default::default();
            state.finish("Previous section jump cancelled.");
            state.selected = next;
        }
        drop(decks);
        self.refresh_section_now(id);
    }

    fn section_ready(&self, id: DeckId) -> Result<(), &'static str> {
        let index = Self::grid_index(id);
        if !self.can_control_deck(id) || self.pending_device_switch.borrow().is_some() {
            return Err("Waiting for the deck or audio output.");
        }
        if self.grid_editors.borrow()[index].is_some() {
            return Err("Save or cancel the grid draft first.");
        }
        if self.pending_pitch.borrow()[index].is_some()
            || self.sync_session.borrow().is_some()
            || self.pending_sync.borrow().is_some()
        {
            return Err("Finish pitch changes and release Sync first.");
        }
        Ok(())
    }

    fn arm_section_jump(&self, id: DeckId) {
        self.refresh_section_now(id);
        let mut decks = self.section_jumps.borrow_mut();
        let state = &mut decks[Self::grid_index(id)];
        if state.pending.is_some() {
            return;
        }
        let result = state
            .selected
            .and_then(|i| state.targets.get(i))
            .ok_or_else(|| "Select a cue or phrase.".to_string())
            .and_then(|t| {
                self.section_ready(id).map_err(str::to_string)?;
                self.engine.borrow().schedule_section_jump(id, t.at)
            });
        match result {
            Ok(ticket) => {
                state.pending = Some(ticket);
                state.notice = "Armed for the next bar.".into();
            }
            Err(e) => {
                state.notice = e.into();
            }
        }
        drop(decks);
        self.refresh_section_now(id);
    }

    pub(super) fn refresh_section_jump(&self, ui: &AppWindow, id: DeckId, snap: &DeckSnapshot) {
        let mut decks = self.section_jumps.borrow_mut();
        let state = &mut decks[Self::grid_index(id)];
        let source_changed = match (state.source.upgrade(), snap.detail.as_ref()) {
            (Some(a), Some(b)) => !Arc::ptr_eq(&a, b),
            (None, None) => !state.targets.is_empty(),
            _ => true,
        };
        if source_changed {
            state.notice = Default::default();
            state.finish("Section jump cancelled. The loaded track changed.");
            state.source = snap.detail.as_ref().map_or_else(Weak::new, Arc::downgrade);
            state.revision = None;
            state.selected = None;
        }
        let library = self.library.borrow();
        let revision = (self.library_generation.get(), library.revision());
        if state.revision != Some(revision) {
            let track = snap
                .path
                .as_deref()
                .and_then(|p| library.track_id_for_path(p))
                .and_then(|i| library.track_by_id(i));
            state.track_id = track.map(|t| t.id);
            let grid = track.and_then(|t| t.grid);
            if state.saved_grid != grid {
                state.finish("Section jump cancelled. The saved grid changed.");
            }
            state.saved_grid = grid;
            state.replace_targets(
                ui,
                id,
                track.map_or_else(Vec::new, |t| targets(t, snap.duration)),
            );
            state.revision = Some(revision);
        }
        if state.pending.is_some() {
            // Prepare can commit a single grid/cue directly without creating a
            // new dirty revision. Compare the eight-slot catalog record by ID.
            let track = state.track_id.and_then(|i| library.track_by_id(i));
            if track.and_then(|t| t.grid) != state.saved_grid {
                state.finish("Section jump cancelled. The saved grid changed.");
            } else if let Some(target) = state.selected.and_then(|i| state.targets.get(i)) {
                let present = track.is_some_and(|t| match target.kind {
                    TargetKind::Cue(slot) => t.hot_cues.slots()[slot]
                        .as_ref()
                        .is_some_and(|c| c.position() == target.at),
                    TargetKind::Phrase => t.metadata.analysis.as_ref().is_some_and(|a| {
                        a.phrase_boundaries_ms
                            .iter()
                            .any(|&ms| Duration::from_millis(u64::from(ms)) == target.at)
                    }),
                });
                if !present {
                    state.finish("Section jump cancelled. The target changed.");
                    state.revision = None;
                }
            }
        }
        drop(library);
        let gate = self.section_ready(id);
        if state.pending.is_some() {
            if state.live_grid != snap.grid || snap.grid.is_none() {
                state.finish("Section jump cancelled. The deck grid changed.");
            } else if !snap.playing {
                state.finish("Section jump cancelled. Playback stopped.");
            } else if let Err(reason) = gate {
                state.finish(reason);
            } else if let Some(ticket) = &state.pending {
                match ticket.status() {
                    SectionJumpStatus::Pending => (),
                    SectionJumpStatus::Executed => {
                        state.pending = None;
                        state.notice = "Section jump executed.".into();
                    }
                    SectionJumpStatus::Cancelled => {
                        state.pending = None;
                        state.notice = "Section jump cancelled by the deck.".into();
                    }
                }
            }
        }
        state.live_grid = snap.grid;
        let target = state.selected.and_then(|i| state.targets.get(i));
        let marker = if state.pending.is_some() {
            target.map_or(-1., |t| {
                (t.at.as_secs_f64() / snap.duration.as_secs_f64()).clamp(0., 1.) as f32
            })
        } else {
            -1.
        };
        let ready: Result<bool, Cow<'_, str>> = if state.pending.is_some() {
            Ok(false)
        } else if !snap.loaded {
            Err("Load a track.".into())
        } else if state.targets.is_empty() {
            Err("No saved cues or analyzed phrase boundaries.".into())
        } else if !snap.playing {
            Err("Start playback to arm a section.".into())
        } else if snap.grid.is_none() {
            Err("Save a beatgrid to arm a section.".into())
        } else if let Some(t) = target {
            gate.map_err(Cow::Borrowed)
                .and_then(|()| {
                    self.engine
                        .borrow()
                        .can_schedule_section_jump(id, t.at)
                        .map_err(Cow::Owned)
                })
                .map(|()| true)
        } else {
            Err("Select a cue or phrase.".into())
        };
        let status = if !state.notice.is_empty() {
            state.notice.as_str()
        } else {
            ready.as_ref().err().map_or("Ready.", AsRef::as_ref)
        };
        let selected = state.selected.map_or(-1, |i| i as i32);
        let can_arm = matches!(ready, Ok(true));
        match id {
            DeckId::A => {
                ui.set_section_target_left(selected);
                ui.set_section_mark_left(marker);
                ui.set_section_can_arm_left(can_arm);
                if ui.get_section_status_left().as_str() != status {
                    ui.set_section_status_left(status.into());
                }
            }
            DeckId::B => {
                ui.set_section_target_right(selected);
                ui.set_section_mark_right(marker);
                ui.set_section_can_arm_right(can_arm);
                if ui.get_section_status_right().as_str() != status {
                    ui.set_section_status_right(status.into());
                }
            }
        }
    }
}
