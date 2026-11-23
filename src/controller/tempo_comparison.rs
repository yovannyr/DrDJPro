//! Half/double-time comparison belongs to the selected Prepare track, independently of both decks.
use super::*;
use crate::{
    beatgrid::{BeatGrid, TempoComparison},
    cancellation::Cancellation,
    playback::{PreviewClip, PREVIEW_DURATION},
};

pub(super) struct Comparison {
    id: i32,
    path: PathBuf,
    draft: TempoComparison,
    fast: bool,
    start: Duration,
    end: Duration,
    clip: Option<PreviewClip>,
    wants_audio: bool,
    route: bool,
    notice: String,
}

// Keep an abandoned read in this slot until it actually exits. Repeated selection
// changes can replace the draft, but never create a second concurrent decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PreviewOwner {
    Tempo,
    Piano,
    Sample,
}

pub(super) struct PreviewJob {
    owner: PreviewOwner,
    epoch: u64,
    cancel: Cancellation,
    result: Receiver<Result<PreviewClip, String>>,
}

impl PreviewJob {
    pub(super) fn start(
        path: PathBuf,
        start: Duration,
        owner: PreviewOwner,
        epoch: u64,
    ) -> Result<Self, String> {
        Self::read(owner, epoch, move |cancel| {
            PreviewClip::open(&path, start, cancel)
        })
    }

    pub(super) fn sample(
        path: PathBuf,
        epoch: u64,
        cached: Option<PreviewClip>,
    ) -> Result<Self, String> {
        Self::read(PreviewOwner::Sample, epoch, move |cancel| {
            PreviewClip::open_sample_or_cached(&path, Duration::ZERO, cancel, cached)
        })
    }

    fn read(
        owner: PreviewOwner,
        epoch: u64,
        read: impl FnOnce(&Cancellation) -> Result<PreviewClip, String> + Send + 'static,
    ) -> Result<Self, String> {
        let cancel = Cancellation::default();
        let worker = cancel.clone();
        let (tx, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("headphone-preview".into())
            .spawn(move || {
                let _ = tx.send(read(&worker));
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            owner,
            epoch,
            cancel,
            result,
        })
    }
}

impl Comparison {
    fn new(track: &crate::library::Track) -> Result<Self, String> {
        let (bpm, anchor) = match track.grid {
            Some(grid) => (grid.bpm(), grid.anchor()),
            None => {
                let analysis = track
                    .metadata
                    .analysis
                    .as_ref()
                    .ok_or("Analyze the selected track or save a grid first")?;
                let ms = analysis
                    .beat_anchor_ms
                    .ok_or("The analysis has no downbeat; mark one in Edit Grid first")?;
                (
                    f64::from(analysis.bpm),
                    Duration::from_millis(u64::from(ms)),
                )
            }
        };
        let start = anchor.saturating_sub(Duration::from_secs(1));
        let end = track
            .metadata
            .duration_ms
            .map(Duration::from_millis)
            .unwrap_or(start + PREVIEW_DURATION)
            .min(start + PREVIEW_DURATION);
        if anchor >= end {
            return Err("The downbeat is outside the selected track".into());
        }
        let draft = TempoComparison::new(track.grid, bpm, anchor)?;
        let fast = draft.pair().1 == 1.0;
        Ok(Self {
            id: track.id,
            path: track.path.clone(),
            draft,
            fast,
            start,
            end,
            clip: None,
            wants_audio: false,
            route: false,
            notice: String::new(),
        })
    }

    fn multiplier(&self) -> f64 {
        let (slow, fast) = self.draft.pair();
        if self.fast {
            fast
        } else {
            slow
        }
    }

    fn grid(&self) -> Result<BeatGrid, String> {
        self.draft.preview(self.multiplier())
    }

    fn view(&self, writable: bool) -> crate::TempoComparisonView {
        let (slow, fast) = self.draft.pair();
        let mut messages = Vec::new();
        if !self.route {
            messages.push("Visual comparison only. Select a separate headphone output to listen.");
        }
        if let Some(reason) = self.draft.blocked_reason() {
            messages.push(reason);
        }
        if !writable {
            messages.push("Library unavailable for saving.");
        }
        if !self.notice.is_empty() {
            messages.push(self.notice.as_str());
        }
        let label = |factor| format!("{:.2} BPM", self.draft.bpm() * factor).into();
        crate::TempoComparisonView {
            open: true,
            track_id: self.id,
            slow_label: label(slow),
            fast_label: label(fast),
            selected_fast: self.fast,
            can_apply: writable && self.draft.blocked_reason().is_none(),
            can_toggle_lock: writable && self.draft.original().is_some(),
            locked: self.draft.original().is_some_and(|g| g.is_locked()),
            anchor_label: format!("Downbeat {:.3} s", self.draft.anchor().as_secs_f64()).into(),
            status: messages.join(" ").into(),
            beats: self
                .grid()
                .map(|g| tempo_lines(g, self.start, self.end))
                .unwrap_or_default(),
        }
    }
}

fn tempo_lines(
    grid: crate::beatgrid::BeatGrid,
    start: Duration,
    end: Duration,
) -> ModelRc<crate::BeatLine> {
    let beat_lines: Vec<crate::BeatLine> = grid
        .lines(start, end)
        .into_iter()
        .map(|l| crate::BeatLine {
            position: l.percent / 100.0,
            downbeat: l.downbeat,
            first: l.first,
            warp_marker: l.warp_marker,
            bar: l.bar,
        })
        .collect();
    ModelRc::new(Rc::new(VecModel::from(beat_lines)))
}

impl AudioController {
    pub(super) fn wire_tempo_comparison(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_tempo_comparison(move || {
            if let Some(c) = weak.upgrade() {
                c.comparison_feedback(c.open_comparison());
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_apply_tempo_comparison(move || {
            if let Some(c) = weak.upgrade() {
                c.comparison_feedback(c.apply_comparison());
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_toggle_tempo_grid_lock(move || {
            if let Some(c) = weak.upgrade() {
                c.comparison_feedback(c.toggle_comparison_lock());
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_cancel_tempo_comparison(move || {
            if let Some(c) = weak.upgrade() {
                c.close_comparison();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_select_tempo_variant(move |fast| {
            if let Some(c) = weak.upgrade() {
                c.comparison_feedback(c.select_comparison(fast));
            }
        });
    }

    fn publish_comparison(&self) {
        if let (Some(ui), Some(cmp)) = (self.ui.upgrade(), self.comparison.borrow().as_ref()) {
            ui.set_tempo_comparison(
                cmp.view(self.storage_ready.get() && self.persistence.borrow().is_some()),
            );
        }
    }

    fn comparison_feedback(&self, result: Result<(), String>) {
        if let Err(error) = result {
            if let Some(cmp) = self.comparison.borrow_mut().as_mut() {
                cmp.notice.clone_from(&error);
            }
            self.publish_comparison();
            if let Some(ui) = self.ui.upgrade() {
                ui.set_status(error.into());
            }
        }
    }

    fn ensure_comparison_current(&self) -> Result<(), String> {
        let current = {
            let guard = self.comparison.borrow();
            let cmp = guard
                .as_ref()
                .ok_or("Open tempo comparison in Prepare first")?;
            let library = self.library.borrow();
            self.ui
                .upgrade()
                .is_some_and(|ui| ui.get_library_tab() == 4 && ui.get_selected_track() == cmp.id)
                && library.selected_id() == cmp.id
                && library
                    .track_by_id(cmp.id)
                    .is_some_and(|track| track.path == cmp.path)
        };
        if !current {
            self.close_comparison();
            return Err("Track changed; reopen tempo comparison".into());
        }
        Ok(())
    }

    fn open_comparison(&self) -> Result<(), String> {
        self.close_clip_check();
        self.close_sample_browser();
        self.close_comparison();
        let ui = self.ui.upgrade().ok_or("Window closed")?;
        self.close_reference_editor();
        let mut cmp = {
            let library = self.library.borrow();
            if ui.get_library_tab() != 4 || ui.get_selected_track() != library.selected_id() {
                return Err("Select a track in Prepare first".into());
            }
            Comparison::new(
                library
                    .track_by_id(library.selected_id())
                    .ok_or("Select a track in Prepare first")?,
            )?
        };
        cmp.route = self.engine.borrow().preview_route_available();
        let fast = cmp.fast;
        *self.comparison.borrow_mut() = Some(cmp);
        self.select_comparison(fast)
    }

    pub(super) fn close_comparison(&self) {
        self.cancel_preview_job(PreviewOwner::Tempo);
        if self.comparison.borrow_mut().take().is_some() {
            self.engine.borrow().stop_tempo_preview();
            if let Some(ui) = self.ui.upgrade() {
                ui.set_tempo_comparison(Default::default());
            }
        }
    }

    fn select_comparison(&self, fast: bool) -> Result<(), String> {
        self.ensure_comparison_current()?;
        let engine = self.engine.borrow();
        engine.stop_tempo_preview();
        {
            let mut guard = self.comparison.borrow_mut();
            let cmp = guard.as_mut().ok_or("Close and reopen tempo comparison")?;
            cmp.fast = fast;
            cmp.notice.clear();
            cmp.route = engine.preview_route_available();
            cmp.wants_audio = cmp.route;
            if cmp.wants_audio {
                if let Some(clip) = &cmp.clip {
                    cmp.wants_audio = false;
                    engine.play_tempo_preview(clip, cmp.grid()?)?;
                } else {
                    cmp.notice = "Loading headphone preview…".into();
                }
            }
        }
        self.publish_comparison();
        self.poll_tempo_comparison();
        Ok(())
    }

    pub(super) fn cancel_preview_job(&self, owner: PreviewOwner) {
        if let Some(job) = self
            .preview_job
            .borrow()
            .as_ref()
            .filter(|job| job.owner == owner)
        {
            job.cancel.request();
        }
    }

    pub(super) fn take_preview_result(
        &self,
        owner: PreviewOwner,
    ) -> Option<(u64, bool, Result<PreviewClip, String>)> {
        let finished = {
            let job = self.preview_job.borrow();
            job.as_ref()
                .filter(|job| job.owner == owner)
                .and_then(|job| match job.result.try_recv() {
                    Ok(result) => Some((job.epoch, job.cancel.is_cancelled(), result)),
                    Err(TryRecvError::Disconnected) => Some((
                        job.epoch,
                        job.cancel.is_cancelled(),
                        Err("Preview worker stopped".into()),
                    )),
                    Err(TryRecvError::Empty) => None,
                })
        };
        if finished.is_some() {
            self.preview_job.borrow_mut().take();
        }
        finished
    }

    pub(super) fn poll_tempo_comparison(&self) {
        let finished = self.take_preview_result(PreviewOwner::Tempo);
        if self.comparison.borrow().is_none() || self.ensure_comparison_current().is_err() {
            return;
        }
        let mut changed = false;
        {
            let engine = self.engine.borrow();
            let mut guard = self.comparison.borrow_mut();
            let Some(cmp) = guard.as_mut() else { return };
            let route = engine.preview_route_available();
            if route != cmp.route {
                cmp.route = route;
                cmp.wants_audio = false;
                cmp.notice.clear();
                self.cancel_preview_job(PreviewOwner::Tempo);
                engine.stop_tempo_preview();
                changed = true;
            }
            if let Some((_, false, result)) = finished {
                if cmp.wants_audio {
                    let played = result.and_then(|clip| {
                        engine.play_tempo_preview(&clip, cmp.grid()?)?;
                        cmp.clip = Some(clip);
                        Ok(())
                    });
                    cmp.wants_audio = false;
                    cmp.notice = played.err().unwrap_or_default();
                    changed = true;
                }
            }
            if cmp.wants_audio && self.preview_job.borrow().is_none() {
                match PreviewJob::start(cmp.path.clone(), cmp.start, PreviewOwner::Tempo, 0) {
                    Ok(job) => *self.preview_job.borrow_mut() = Some(job),
                    Err(error) => {
                        cmp.wants_audio = false;
                        cmp.notice = error;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.publish_comparison();
        }
    }

    fn apply_comparison(&self) -> Result<(), String> {
        self.ensure_comparison_current()?;
        {
            let guard = self.comparison.borrow();
            let cmp = guard.as_ref().ok_or("Open tempo comparison first")?;
            let candidate = cmp
                .draft
                .apply_candidate(cmp.multiplier(), self.library.borrow().grid(&cmp.path))?;
            self.persistence
                .borrow()
                .as_ref()
                .ok_or("Library unavailable for saving")?
                .commit_prepare_grid(cmp.id, &cmp.path, cmp.draft.original(), candidate)?;
            self.library
                .borrow_mut()
                .set_grid(&cmp.path, Some(candidate))?;
        }
        self.library_changed();
        self.close_comparison();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_status("Tempo and downbeat saved".into());
        }
        Ok(())
    }

    fn toggle_comparison_lock(&self) -> Result<(), String> {
        self.ensure_comparison_current()?;
        {
            let mut guard = self.comparison.borrow_mut();
            let cmp = guard.as_mut().ok_or("Open tempo comparison first")?;
            let grid = cmp.draft.original().ok_or("Save a grid first")?;
            if self.library.borrow().grid(&cmp.path) != Some(grid) {
                return Err("Saved grid changed; reopen tempo comparison".into());
            }
            let locked = !grid.is_locked();
            self.persistence
                .borrow()
                .as_ref()
                .ok_or("Library unavailable for saving")?
                .set_prepare_grid_lock(cmp.id, &cmp.path, grid, locked)?;
            self.library
                .borrow_mut()
                .set_grid_locked(&cmp.path, locked)?;
            cmp.draft =
                TempoComparison::new(Some(grid.with_locked(locked)), grid.bpm(), grid.anchor())?;
            cmp.notice.clear();
        }
        self.library_changed();
        self.publish_comparison();
        Ok(())
    }
}
