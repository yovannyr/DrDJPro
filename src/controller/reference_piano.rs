//! One Prepare editor owns its bounded passage read and its headphones-only reference voice.
use super::tempo_comparison::{PreviewJob, PreviewOwner};
use super::*;
use crate::{playback::PreviewClip, track_metadata::MusicLabels};
use drdjpro_ui_spike::analysis::keys::CamelotKey;

#[derive(Clone, Copy)]
struct Audition {
    start: Duration,
    key: CamelotKey,
    triad: bool,
}
pub(super) struct PianoDraft {
    track_id: i32,
    path: PathBuf,
    generation: u64,
    epoch: u64,
    request: Option<Audition>,
    playing: bool,
    clip: Option<(Duration, PreviewClip)>,
}

impl PianoDraft {
    fn current(&self, c: &AudioController, ui: &AppWindow) -> bool {
        let editor = ui.get_cover_label_editor();
        if !ui.get_reference_editor_visible()
            || editor.track_id != self.track_id
            || ui.get_selected_track() != self.track_id
            || !c.storage_ready.get()
            || c.library_generation.get() != self.generation
        {
            return false;
        }
        let library = c.library.borrow();
        library.selected_id() == self.track_id
            && library
                .track_by_id(self.track_id)
                .is_some_and(|t| t.path == self.path)
    }
    fn stop(&mut self) {
        self.request = None;
        self.playing = false;
        self.epoch = self.epoch.wrapping_add(1);
    }
}

impl AudioController {
    pub(super) fn wire_reference_piano(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_audition_reference(move |id, key, start, triad| {
            if let Some(c) = weak.upgrade() {
                if let Err(error) = c.request_reference(id, &key, &start, triad) {
                    c.stop_reference(&error);
                }
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_stop_reference(move || {
            if let Some(c) = weak.upgrade() {
                c.stop_reference("Reference stopped.");
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_reset_confirmed_key(move |id| {
            if let Some(c) = weak.upgrade() {
                if let Some(ui) = c.ui.upgrade() {
                    if let Err(error) = c.reset_confirmed_key(&ui, id) {
                        ui.set_status(error.into());
                    }
                }
            }
        });
    }

    pub(super) fn begin_reference_editor(&self, ui: &AppWindow, track: &crate::library::Track) {
        *self.reference.borrow_mut() = Some(PianoDraft {
            track_id: track.id,
            path: track.path.clone(),
            generation: self.library_generation.get(),
            epoch: 0,
            request: None,
            playing: false,
            clip: None,
        });
        let route = self.engine.borrow().preview_route_available();
        ui.set_reference_route(route);
        ui.set_reference_active(false);
        ui.set_reference_status(
            if route {
                "Choose a passage and play the reference on headphones."
            } else {
                "A separate headphone output (3/4) is required to audition."
            }
            .into(),
        );
    }
    pub(super) fn reference_current(&self, ui: &AppWindow, id: i32) -> bool {
        self.reference
            .borrow()
            .as_ref()
            .is_some_and(|d| d.track_id == id && d.current(self, ui))
    }
    pub(super) fn stop_reference(&self, message: &str) {
        let mut draft = self.reference.borrow_mut();
        let Some(draft) = draft.as_mut() else { return };
        draft.stop();
        self.cancel_preview_job(PreviewOwner::Piano);
        self.engine.borrow().stop_tempo_preview();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_reference_active(false);
            ui.set_reference_route(self.engine.borrow().preview_route_available());
            ui.set_reference_status(message.into());
        }
    }
    pub(super) fn close_reference_editor(&self) {
        if self.reference.borrow_mut().take().is_some() {
            self.cancel_preview_job(PreviewOwner::Piano);
            self.engine.borrow().stop_tempo_preview();
            if let Some(ui) = self.ui.upgrade() {
                ui.set_cover_label_editor(Default::default());
                ui.set_reference_active(false);
                ui.set_reference_route(false);
                ui.set_reference_status(Default::default());
            }
        }
    }
    fn request_reference(
        &self,
        id: i32,
        key: &str,
        seconds: &str,
        triad: bool,
    ) -> Result<(), String> {
        let ui = self.ui.upgrade().ok_or("Window closed")?;
        if !self.reference_current(&ui, id) {
            // A queued event for a different row must not silence its successor.
            ui.set_status("Select this track in Prepare again.".into());
            return Ok(());
        }
        if !self.engine.borrow().preview_route_available() {
            return Err("A separate headphone output (3/4) is required to audition.".into());
        }
        let key = MusicLabels::parse(key, "")?
            .key()
            .ok_or("Choose a Camelot key to audition.")?;
        let end = self
            .library
            .borrow()
            .track_by_id(id)
            .and_then(|t| t.metadata.duration_ms);
        let start = passage_start(seconds, end)?;
        self.stop_reference("Loading headphone passage…");
        {
            let mut reference = self.reference.borrow_mut();
            let draft = reference.as_mut().ok_or("Reference editor closed")?;
            if draft.clip.as_ref().is_some_and(|(at, _)| *at != start) {
                draft.clip = None;
            }
            draft.request = Some(Audition { start, key, triad });
        }
        ui.set_reference_active(true);
        self.poll_reference();
        Ok(())
    }

    pub(super) fn poll_reference(&self) {
        let finished = self.take_preview_result(PreviewOwner::Piano);
        let Some(ui) = self.ui.upgrade() else { return };
        if self.reference.borrow().is_none() {
            return;
        }
        if !self
            .reference
            .borrow()
            .as_ref()
            .is_some_and(|d| d.current(self, &ui))
        {
            self.close_reference_editor();
            return;
        }
        let route = self.engine.borrow().preview_route_available();
        if ui.get_reference_route() != route {
            if !route {
                self.stop_reference("A separate headphone output (3/4) is required to audition.");
            } else {
                ui.set_reference_status(
                    "Headphone output available. Choose Play to audition.".into(),
                );
            }
            ui.set_reference_route(route);
        }
        let mut state = self.reference.borrow_mut();
        let Some(draft) = state.as_mut() else { return };
        let Some(request) = draft.request else { return };
        if !route || draft.playing {
            return;
        }
        if let Some((epoch, false, result)) = finished {
            if epoch == draft.epoch {
                match result {
                    Ok(clip) => draft.clip = Some((request.start, clip)),
                    Err(e) => {
                        draft.stop();
                        ui.set_reference_active(false);
                        ui.set_reference_status(e.into());
                        return;
                    }
                }
            }
        }
        if let Some((_, clip)) = &draft.clip {
            match self
                .engine
                .borrow()
                .play_key_reference(clip, request.key, request.triad)
            {
                Ok(()) => {
                    draft.playing = true;
                    ui.set_reference_status(
                        format!(
                            "Looping from {:.2} s with {} {} on headphones.",
                            request.start.as_secs_f64(),
                            request.key,
                            if request.triad { "triad" } else { "root" }
                        )
                        .into(),
                    );
                }
                Err(e) => {
                    draft.stop();
                    ui.set_reference_active(false);
                    ui.set_reference_status(e.into());
                }
            }
        } else if self.preview_job.borrow().is_none() {
            match PreviewJob::start(
                draft.path.clone(),
                request.start,
                PreviewOwner::Piano,
                draft.epoch,
            ) {
                Ok(job) => *self.preview_job.borrow_mut() = Some(job),
                Err(e) => {
                    draft.stop();
                    ui.set_reference_active(false);
                    ui.set_reference_status(e.into());
                }
            }
        }
    }

    fn reset_confirmed_key(&self, ui: &AppWindow, id: i32) -> Result<(), String> {
        if !self.reference_current(ui, id) {
            return Err("Track changed. Reopen its key in Prepare.".into());
        }
        let labels = self
            .library
            .borrow()
            .track_by_id(id)
            .ok_or("Track unavailable")?
            .metadata
            .music_labels;
        let updated = MusicLabels::from_stored(None, labels.energy().map(u32::from))?;
        self.stop_reference("Reference stopped.");
        self.write_music_labels(ui, id, updated)?;
        self.open_cover_labels(ui)
    }
}

fn passage_start(raw: &str, duration_ms: Option<u64>) -> Result<Duration, String> {
    let seconds = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| "Enter a passage start in seconds.")?;
    if !(0.0..=604800.).contains(&seconds) {
        return Err("Passage start must be between zero and seven days.".into());
    }
    let start = Duration::try_from_secs_f64(seconds).map_err(|_| "Invalid passage start.")?;
    if duration_ms.is_some_and(|end| start >= Duration::from_millis(end)) {
        return Err("Passage starts outside this track.".into());
    }
    Ok(start)
}
