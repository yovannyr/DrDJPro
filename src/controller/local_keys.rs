//! Local key estimates are read on demand, independently of the global track labels or deck transport.
use super::*;
use crate::{
    beatgrid::BeatGrid,
    cancellation::Cancellation,
    library_store::LocalKeyCache,
    local_harmony::{self, KeySection, LocalKeyProfile},
    playback::SampleVersion,
};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(PartialEq, Eq)]
pub(super) enum Version {
    Native(SampleVersion),
    Digest([u8; 32]),
}

impl Version {
    pub(super) fn read(path: &Path, cancel: &Cancellation) -> Result<Self, String> {
        cancel.check()?;
        match SampleVersion::read(path, Duration::ZERO)? {
            Some(stamp) => Ok(Self::Native(stamp)),
            None => local_harmony::fingerprint(path, cancel).map(Self::Digest),
        }
    }
}

fn load_profile(
    id: i32,
    path: &Path,
    cache: Option<&LocalKeyCache>,
    version: &Version,
    cancel: &Cancellation,
) -> Result<(LocalKeyProfile, String), String> {
    let digest = match version {
        Version::Digest(digest) => *digest,
        Version::Native(_) => local_harmony::fingerprint(path, cancel)?,
    };
    let mut notice = String::new();
    if let Some(cache) = cache {
        match cache.lookup(id, path, &digest) {
            Ok(Some(profile))
                if profile.source_sha256 == digest
                    && profile.algorithm_version == local_harmony::ALGORITHM_VERSION =>
            {
                if Version::read(path, cancel)? != *version {
                    return Err("Source changed during profile verification.".into());
                }
                return Ok((profile, "Restored verified local keys.".into()));
            }
            Err(e) => notice = format!(" Could not read the cached profile: {e}"),
            _ => (),
        }
    }
    cancel.check()?;
    let profile = local_harmony::analyze(path, cancel)?;
    if profile.source_sha256 != digest || Version::read(path, cancel)? != *version {
        return Err("Source changed during local key analysis.".into());
    }
    if let Some(cache) = cache {
        cancel.check()?;
        if let Err(e) = cache.save(id, path, &profile) {
            notice = format!(" Session only: {e}");
        }
    } else {
        notice = " Session only.".into();
    }
    Ok((profile, format!("Local keys analyzed.{notice}")))
}

enum Event {
    Changed,
    Loaded(LocalKeyProfile, String, u64),
    Verified(u64),
    Failed(String),
}

struct Job {
    cancel: Cancellation,
    wake: mpsc::SyncSender<()>,
    sequence: Arc<AtomicU64>,
    events: Receiver<Event>,
}

impl Job {
    fn start(id: i32, path: PathBuf, cache: Option<LocalKeyCache>) -> Result<Self, String> {
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (wake, requests) = mpsc::sync_channel(1);
        // A suspended window cannot accumulate profiles while files are being replaced.
        let (tx, events) = mpsc::sync_channel(2);
        let sequence = Arc::new(AtomicU64::new(0));
        let requested = Arc::clone(&sequence);
        std::thread::Builder::new()
            .name("local-key-profile".into())
            .spawn(move || {
                let mut last = None;
                let mut explicit = true;
                while !token.is_cancelled() {
                    let seq = requested.load(Ordering::Acquire);
                    match Version::read(&path, &token) {
                        Ok(stamp) if last.as_ref() == Some(&stamp) => {
                            if explicit && tx.send(Event::Verified(seq)).is_err() {
                                break;
                            }
                        }
                        Ok(stamp) => {
                            if tx.send(Event::Changed).is_err() {
                                break;
                            }
                            match load_profile(id, &path, cache.as_ref(), &stamp, &token) {
                                Ok((profile, note)) => {
                                    last = Some(stamp);
                                    if tx.send(Event::Loaded(profile, note, seq)).is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(Event::Failed(e));
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Failed(e));
                            break;
                        }
                    }
                    // Reliable file identity is cheap to check. On other filesystems an idle
                    // refresh requires reading the bytes, so reduce its frequency.
                    let seconds = if matches!(last, Some(Version::Native(_))) {
                        1
                    } else {
                        30
                    };
                    explicit = match requests.recv_timeout(Duration::from_secs(seconds)) {
                        Ok(()) => true,
                        Err(mpsc::RecvTimeoutError::Timeout) => false,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            cancel,
            wake,
            sequence,
            events,
        })
    }
    fn verify(&self) -> u64 {
        let seq = self.sequence.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        let _ = self.wake.try_send(());
        seq
    }
    fn stop(&self) {
        self.cancel.request();
        let _ = self.wake.try_send(());
    }
}

struct Session {
    id: i32,
    path: PathBuf,
    generation: u64,
    started: bool,
    sequence: u64,
    profile: Option<Arc<LocalKeyProfile>>,
    note: String,
    verified: bool,
    entry: String,
    exit: String,
    last_grid: Option<BeatGrid>,
    revision: Option<u64>,
}
#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    job: Option<Job>,
}

fn point(raw: &str, duration: u64, exit: bool) -> Result<u64, &'static str> {
    if duration == 0 {
        return Err("Empty track");
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(if exit { duration - 1 } else { 0 });
    }
    let seconds = raw
        .parse::<f64>()
        .ok()
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
        .ok_or("Enter non-negative seconds")?;
    let end = Duration::from_millis(duration);
    if seconds > end || (!exit && seconds == end) {
        return Err("Point is outside the track");
    }
    Ok((seconds.as_millis() as u64).min(duration - 1))
}

fn key_text(section: &KeySection) -> String {
    let Some(raw) = section.key.as_deref() else {
        return format!("Unknown ({}%)", section.confidence);
    };
    let note = drdjpro_ui_spike::analysis::keys::CamelotKey::parse(raw)
        .map(|key| key.to_standard())
        .unwrap_or_default();
    format!("{note} / {raw} ({}%)", section.confidence)
}

fn position(ms: u64, grid: Option<BeatGrid>) -> String {
    let sec = format!("{}.{:03} s", ms / 1000, ms % 1000);
    let Some(grid) = grid else {
        return sec;
    };
    let (bar, beat) = grid.bar_beat_at_ms(ms);
    format!("{sec} · bar {bar} beat {beat}")
}

fn point_text(raw: &str, profile: &LocalKeyProfile, exit: bool, grid: Option<BeatGrid>) -> String {
    let ms = match point(raw, profile.duration_ms, exit) {
        Ok(ms) => ms,
        Err(e) => return e.into(),
    };
    let index = profile.segments.partition_point(|s| s.end_ms <= ms);
    let key = profile
        .segments
        .get(index)
        .filter(|s| s.start_ms <= ms)
        .map_or_else(|| "Unknown".into(), key_text);
    format!("{} · {key}", position(ms, grid))
}

/// The window asks for visible rows only. Moving a point never reformats a whole track.
struct SectionRows {
    profile: Arc<LocalKeyProfile>,
    grid: Option<BeatGrid>,
    notify: slint::ModelNotify,
}
impl Model for SectionRows {
    type Data = slint::SharedString;
    fn row_count(&self) -> usize {
        self.profile.segments.len()
    }
    fn row_data(&self, row: usize) -> Option<Self::Data> {
        let s = self.profile.segments.get(row)?;
        Some(
            format!(
                "{} – {} · {}",
                position(s.start_ms, self.grid),
                position(s.end_ms, self.grid),
                key_text(s)
            )
            .into(),
        )
    }
    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
}

impl Session {
    fn render(&mut self, ui: &AppWindow, grid: Option<BeatGrid>, revision: u64) {
        let Some(profile) = &self.profile else { return };
        if !self.verified {
            return;
        }
        if self.revision.is_none() || self.last_grid != grid {
            ui.set_local_keys_sections(ModelRc::new(SectionRows {
                profile: Arc::clone(profile),
                grid,
                notify: Default::default(),
            }));
        }
        self.last_grid = grid;
        self.revision = Some(revision);
        ui.set_local_keys_entry(point_text(&self.entry, profile, false, grid).into());
        ui.set_local_keys_exit(point_text(&self.exit, profile, true, grid).into());
        let status = if grid.is_some() {
            self.note.clone()
        } else {
            format!("{} No saved beatgrid; positions are in seconds.", self.note)
        };
        ui.set_local_keys_status(status.into());
        ui.set_local_keys_busy(false);
    }
}

fn clear(ui: &AppWindow) {
    ui.set_local_keys_sections(Default::default());
    ui.set_local_keys_entry(Default::default());
    ui.set_local_keys_exit(Default::default());
}

impl AudioController {
    pub(super) fn wire_local_keys(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_local_keys(move || {
            if let Some(c) = weak.upgrade() {
                c.open_local_keys();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_close_local_keys(move || {
            if let Some(c) = weak.upgrade() {
                c.close_local_keys();
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_select_local_key_times(move |entry, exit| {
            if let Some(c) = weak.upgrade() {
                c.select_local_key_times(entry.to_string(), exit.to_string());
            }
        });
    }
    fn open_local_keys(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let id = ui.get_selected_track();
        let library = self.library.borrow();
        if ui.get_library_tab() != 4
            || ui.get_import_review_open()
            || !self.storage_ready.get()
            || library.selected_id() != id
        {
            return;
        }
        let Some(track) = library.track_by_id(id) else {
            ui.set_status("Select a track in Prepare.".into());
            return;
        };
        let path = track.path.clone();
        drop(library);
        self.close_clip_check();
        self.close_reference_editor();
        self.close_comparison();
        self.close_metadata_diagnosis();
        self.close_sample_browser();
        self.close_local_keys();
        let session = Session {
            id,
            path,
            generation: self.library_generation.get(),
            started: false,
            sequence: 0,
            profile: None,
            note: String::new(),
            verified: false,
            entry: String::new(),
            exit: String::new(),
            last_grid: None,
            revision: None,
        };
        self.local_keys.borrow_mut().session = Some(session);
        clear(&ui);
        ui.set_local_keys_track(id);
        ui.set_local_keys_busy(true);
        ui.set_local_keys_status("Checking the audio file and local key profile…".into());
        ui.set_local_keys_open(true);
        self.poll_local_keys();
    }

    pub(super) fn close_local_keys(&self) {
        let mut state = self.local_keys.borrow_mut();
        if state.session.take().is_none() {
            return;
        }
        if let Some(job) = &state.job {
            job.stop();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_local_keys_open(false);
            clear(&ui);
            ui.set_local_keys_busy(false);
            ui.set_local_keys_track(-1);
            ui.set_local_keys_status(Default::default());
        }
    }

    fn select_local_key_times(&self, entry: String, exit: String) {
        let Some(ui) = self.ui.upgrade() else { return };
        if !ui.get_local_keys_visible() {
            return;
        }
        let mut state = self.local_keys.borrow_mut();
        let seq = state
            .job
            .as_ref()
            .filter(|job| !job.cancel.is_cancelled())
            .map(Job::verify);
        let Some(s) = &mut state.session else { return };
        s.entry = entry;
        s.exit = exit;
        if let Some(seq) = seq {
            s.sequence = seq;
            s.verified = false;
            s.revision = None;
            clear(&ui);
            ui.set_local_keys_busy(true);
            ui.set_local_keys_status("Verifying the selected source…".into());
        }
    }

    pub(super) fn poll_local_keys(&self) {
        {
            let state = self.local_keys.borrow();
            if state.session.is_none() && state.job.is_none() {
                return;
            }
        }
        let Some(ui) = self.ui.upgrade() else { return };
        let stale = self.local_keys.borrow().session.as_ref().is_some_and(|s| {
            let library = self.library.borrow();
            !ui.get_local_keys_visible()
                || !self.storage_ready.get()
                || self.library_generation.get() != s.generation
                || library.selected_id() != s.id
                || library.track_by_id(s.id).is_none_or(|t| t.path != s.path)
        });
        if stale {
            self.close_local_keys();
        }
        let mut state = self.local_keys.borrow_mut();
        while let Some(job) = &state.job {
            match job.events.try_recv() {
                Ok(_) if job.cancel.is_cancelled() => continue,
                Ok(event) => {
                    let Some(s) = &mut state.session else {
                        continue;
                    };
                    match event {
                        Event::Changed => {
                            clear(&ui);
                            s.profile = None;
                            s.verified = false;
                            s.revision = None;
                            ui.set_local_keys_busy(true);
                            ui.set_local_keys_status(
                                "Checking the audio file and local key profile…".into(),
                            );
                        }
                        Event::Loaded(profile, note, seq) => {
                            s.profile = Some(Arc::new(profile));
                            s.note = note;
                            s.verified = seq == s.sequence;
                            s.revision = None;
                        }
                        Event::Verified(seq) => {
                            if seq == s.sequence {
                                s.verified = true;
                                s.revision = None;
                            }
                        }
                        Event::Failed(e) => {
                            clear(&ui);
                            s.profile = None;
                            s.verified = false;
                            ui.set_local_keys_busy(false);
                            ui.set_local_keys_status(e.into());
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !job.cancel.is_cancelled()
                        && ui.get_local_keys_busy()
                        && state.session.is_some()
                    {
                        clear(&ui);
                        ui.set_local_keys_busy(false);
                        ui.set_local_keys_status(
                            "Local key worker stopped. Reopen the profile to retry.".into(),
                        );
                    }
                    state.job = None;
                    break;
                }
            }
        }
        let has_job = state.job.is_some();
        let Some(s) = &mut state.session else { return };
        if !s.started && !has_job {
            s.started = true;
            let cache = self
                .persistence
                .borrow()
                .as_ref()
                .map(|w| w.local_key_cache());
            match Job::start(s.id, s.path.clone(), cache) {
                Ok(job) => state.job = Some(job),
                Err(e) => {
                    ui.set_local_keys_busy(false);
                    ui.set_local_keys_status(e.into());
                }
            }
        } else if s.verified {
            let library = self.library.borrow();
            if s.revision != Some(library.revision()) {
                let grid = library.track_by_id(s.id).and_then(|t| t.grid);
                s.render(&ui, grid, library.revision());
            }
        }
    }
}
