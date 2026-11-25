//! Modeless arrangement editor. Only worker jobs open media or write project/export files.
use super::*;
use crate::{
    cancellation::Cancellation,
    set_timeline::{self as timeline, Clip, Point, Project},
    TimelineRange, TimelineRow, TimelineWindow,
};

enum Output {
    Opened(Project, PathBuf, Vec<bool>),
    Saved(PathBuf),
    Sources(Vec<bool>),
    Relink(usize, PathBuf),
    Audio(crate::playback::PreviewClip),
    Exported(PathBuf),
}
struct Job {
    cancel: Cancellation,
    rx: Receiver<Result<Output, String>>,
}
pub(super) struct State {
    project: Project,
    path: Option<PathBuf>,
    missing: Vec<bool>,
    selected: Option<usize>,
    curve: i32,
    point: Option<usize>,
    view: Option<TimelineWindow>,
    dirty: bool,
    job: Option<Job>,
    monitor: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
    notice: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            project: Project::new(),
            path: None,
            missing: Vec::new(),
            selected: None,
            curve: 0,
            point: None,
            view: None,
            dirty: false,
            job: None,
            monitor: None,
            notice: String::new(),
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.cancel.request();
        }
        self.stop_audio();
    }
}
fn seconds(ms: u64) -> String {
    format!("{:.3}", ms as f64 / 1000.)
}
fn milliseconds(s: &str, label: &str) -> Result<u64, String> {
    let n = s
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=604800.).contains(v))
        .ok_or_else(|| format!("Enter valid {label} in seconds."))?;
    Ok((n * 1000.).round() as u64)
}
fn curve_mut(c: &mut Clip, k: i32) -> &mut Vec<Point> {
    match k {
        1 => &mut c.eq,
        2 => &mut c.fader,
        _ => &mut c.gain,
    }
}
fn curve(c: &Clip, k: i32) -> &[Point] {
    match k {
        1 => &c.eq,
        2 => &c.fader,
        _ => &c.gain,
    }
}
fn interval(a: &str, b: &str, duration: u64) -> Result<(u64, u64), String> {
    let start = if a.trim().is_empty() {
        0
    } else {
        milliseconds(a, "mix start")?
    };
    let end = if b.trim().is_empty() {
        duration
    } else {
        milliseconds(b, "mix end")?
    };
    if start >= end || end > duration {
        return Err("Mark a non-empty interval inside the arrangement.".into());
    }
    Ok((start, end))
}
fn missing_sources(project: &Project, c: &Cancellation) -> Result<Vec<bool>, String> {
    project
        .clips
        .iter()
        .map(|clip| {
            c.check()?;
            Ok(!clip.path.is_file())
        })
        .collect()
}
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(x, y)| x == y)
}
fn destination(path: PathBuf, ext: &str) -> Result<PathBuf, String> {
    if path.extension().is_none() {
        return Ok(path.with_extension(ext));
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case(ext))
    {
        return Ok(path);
    }
    Err(format!("Choose a .{ext} destination."))
}
impl State {
    fn view(&self) -> Result<&TimelineWindow, String> {
        self.view
            .as_ref()
            .ok_or_else(|| "Timeline window closed.".into())
    }
    fn stop_audio(&mut self) {
        if let Some((weak, epoch)) = self.monitor.take() {
            if let Some(e) = weak.upgrade().filter(|e| e.preview_epoch() == epoch) {
                e.stop_tempo_preview();
            }
        }
    }
    fn halt(&mut self) {
        self.stop_audio();
        if let Some(job) = &self.job {
            job.cancel.request();
            self.notice = "Cancelling…".into();
        }
    }
    fn idle(&self) -> Result<(), String> {
        if self.job.is_some() {
            Err("Finish or cancel the current operation.".into())
        } else {
            Ok(())
        }
    }
    fn index(&self) -> Result<usize, String> {
        self.selected
            .filter(|&i| i < self.project.clips.len())
            .ok_or_else(|| "Choose a clip on the timeline.".into())
    }
    fn start(
        &mut self,
        notice: &str,
        work: impl FnOnce(&Cancellation) -> Result<Output, String> + Send + 'static,
    ) -> Result<(), String> {
        self.idle()?;
        self.stop_audio();
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("set-timeline".into())
            .spawn(move || {
                let _ = tx.send(work(&token));
            })
            .map_err(|e| e.to_string())?;
        self.job = Some(Job { cancel, rx });
        self.notice = notice.into();
        Ok(())
    }
    fn change(
        &mut self,
        work: impl FnOnce(&mut Project) -> Result<(), String>,
    ) -> Result<(), String> {
        self.idle()?;
        let mut next = self.project.clone();
        work(&mut next)?;
        next.validate()?;
        self.stop_audio();
        self.project = next;
        self.dirty = true;
        Ok(())
    }
    fn refresh(&self) {
        let Some(v) = &self.view else {
            return;
        };
        v.set_project_name(self.path.as_ref().and_then(|p| p.file_name()).map_or_else(
            || "Untitled arrangement".into(),
            |p| p.to_string_lossy().into_owned().into(),
        ));
        v.set_dirty(self.dirty);
        v.set_project_end(self.project.end_ms() as i32);
        v.set_tempo(self.project.bpm.to_string().into());
        let end = self.project.end_ms().max(1) as f64;
        v.set_clips(ModelRc::new(VecModel::from(
            self.project
                .clips
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let missing = self.missing.get(i) == Some(&true);
                    TimelineRow {
                        label: format!(
                            "{}–{} s · {}{} · source {} s",
                            seconds(c.start_ms),
                            seconds(c.start_ms + c.length_ms),
                            c.title,
                            if missing { " · SOURCE MISSING" } else { "" },
                            seconds(c.source_ms)
                        )
                        .into(),
                        position: (c.start_ms as f64 / end) as f32,
                        fraction: (c.length_ms as f64 / end) as f32,
                        missing,
                    }
                })
                .collect::<Vec<_>>(),
        )));
        v.set_selected(self.selected.map_or(-1, |i| i as i32));
        v.set_curve_index(self.curve);
        if let Some(c) = self.selected.and_then(|i| self.project.clips.get(i)) {
            v.set_source_path(c.path.to_string_lossy().as_ref().into());
            v.set_clip_at(seconds(c.start_ms).into());
            v.set_source_at(seconds(c.source_ms).into());
            v.set_length(seconds(c.length_ms).into());
            v.set_first_beat(c.first_beat_ms.map(seconds).unwrap_or_default().into());
            v.set_native_bpm(
                c.source_bpm
                    .map(|b| b.to_string())
                    .unwrap_or_default()
                    .into(),
            );
            let pts = curve(c, self.curve);
            v.set_points(ModelRc::new(VecModel::from(
                pts.iter()
                    .map(|p| format!("{} s · {}", seconds(p.ms), p.value).into())
                    .collect::<Vec<slint::SharedString>>(),
            )));
            v.set_point_index(self.point.map_or(-1, |i| i as i32));
            v.set_point_at(
                self.point
                    .and_then(|i| pts.get(i))
                    .map_or_else(|| "0".into(), |p| seconds(p.ms).into()),
            );
            v.set_point_value(self.point.and_then(|i| pts.get(i)).map_or_else(
                || {
                    if self.curve == 2 {
                        "1".into()
                    } else {
                        "0".into()
                    }
                },
                |p| p.value.to_string().into(),
            ));
        }
    }
}
impl AudioController {
    pub(super) fn wire_timeline_entry(self: &Rc<Self>, planning: &crate::SetPlanningWindow) {
        let weak = Rc::downgrade(self);
        planning.on_open_project(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_timeline() {
                    if let Some(ui) = c.ui.upgrade() {
                        ui.set_status(e.into());
                    }
                }
            }
        });
    }
    fn open_timeline(self: &Rc<Self>) -> Result<(), String> {
        if let Some(v) = &self.timeline.borrow().view {
            return v.show().map_err(|e| e.to_string());
        }
        let v = TimelineWindow::new().map_err(|e| e.to_string())?;
        v.on_describe_range(
            |a, b, duration| match interval(&a, &b, duration.max(0) as u64) {
                Ok((start, end)) => TimelineRange {
                    label: format!(
                        "{}–{} s · {} s selected",
                        seconds(start),
                        seconds(end),
                        seconds(end - start)
                    )
                    .into(),
                    left: (start as f64 / f64::from(duration)) as f32,
                    span: ((end - start) as f64 / f64::from(duration)) as f32,
                    valid: true,
                },
                Err(e) => TimelineRange {
                    label: if duration > 0 {
                        e.into()
                    } else {
                        "No clips".into()
                    },
                    ..Default::default()
                },
            },
        );
        macro_rules! wire{
            ($cb:ident,$method:ident $(,$arg:ident)*)=>{{
                let weak=Rc::downgrade(self);
                v.$cb(move|$($arg),*|{if let Some(c)=weak.upgrade(){
                    if let Err(e)=c.$method($($arg),*){c.timeline.borrow_mut().notice=e;}
                    c.poll_timeline();
                }});
            }}
        }
        wire!(on_project_file, timeline_file, action);
        wire!(on_add_clip, add_timeline_clip);
        wire!(on_select_clip, select_timeline_clip, i);
        wire!(on_edit_clip, edit_timeline_clip);
        wire!(on_remove_clip, remove_timeline_clip);
        wire!(on_relink, relink_timeline);
        wire!(on_check_sources, check_timeline);
        wire!(on_set_tempo, tempo_timeline);
        wire!(on_select_curve, select_timeline_curve, i);
        wire!(on_select_point, select_timeline_point, i);
        wire!(on_write_point, write_timeline_point);
        wire!(on_remove_point, remove_timeline_point);
        wire!(on_mark_clip, mark_timeline_clip);
        wire!(on_mark_overlap, mark_timeline_overlap);
        wire!(on_render, render_timeline, mode, motion);
        let weak = Rc::downgrade(self);
        v.on_halt(move || {
            if let Some(c) = weak.upgrade() {
                c.timeline.borrow_mut().halt();
            }
        });
        let weak = Rc::downgrade(self);
        v.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_timeline();
            }
        });
        let weak = Rc::downgrade(self);
        v.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_timeline();
            }
            slint::CloseRequestResponse::HideWindow
        });
        v.show().map_err(|e| e.to_string())?;
        let mut st = self.timeline.borrow_mut();
        st.view = Some(v);
        st.refresh();
        drop(st);
        self.poll_timeline();
        Ok(())
    }
    pub(super) fn close_timeline(&self) {
        let mut st = self.timeline.borrow_mut();
        st.halt();
        if let Some(v) = st.view.take() {
            let _ = v.hide();
        }
    }
    fn timeline_file(&self, action: i32) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        st.idle()?;
        match action {
            0 => {
                st.stop_audio();
                st.project = Project::new();
                st.path = None;
                st.missing.clear();
                st.selected = None;
                st.point = None;
                st.dirty = false;
                st.notice = "New arrangement.".into();
                if let Some(v) = &st.view {
                    v.set_mark_in("".into());
                    v.set_mark_out("".into());
                }
                st.refresh();
                Ok(())
            }
            1 => st.start("Opening project…", move |c| {
                let path = rfd::FileDialog::new()
                    .set_title("Open arrangement")
                    .add_filter("Set arrangement", &["drdjset"])
                    .pick_file()
                    .ok_or("Open cancelled.")?;
                c.check()?;
                let project = Project::load(&path, c)?;
                let missing = missing_sources(&project, c)?;
                Ok(Output::Opened(project, path, missing))
            }),
            2 | 3 => {
                let path = if action == 2 { st.path.clone() } else { None };
                let project = st.project.clone();
                st.start("Saving project…", move |c| {
                    let path = match path {
                        Some(p) => p,
                        None => destination(
                            rfd::FileDialog::new()
                                .set_title("Save arrangement")
                                .add_filter("Set arrangement", &["drdjset"])
                                .set_file_name("set.drdjset")
                                .save_file()
                                .ok_or("Save cancelled.")?,
                            "drdjset",
                        )?,
                    };
                    c.check()?;
                    project.save(&path)?;
                    Ok(Output::Saved(path))
                })
            }
            _ => Err("Unknown project action.".into()),
        }
    }
    fn add_timeline_clip(&self) -> Result<(), String> {
        let ui = self.ui.upgrade().ok_or("Library closed.")?;
        let lib = self.library.borrow();
        if lib.selected_id() != ui.get_selected_track() {
            return Err("Choose a track in the main library.".into());
        }
        let t = lib
            .track_by_id(lib.selected_id())
            .ok_or("Choose a track in the main library.")?;
        let mut st = self.timeline.borrow_mut();
        let start = st.project.end_ms();
        let p = lib.planning_track(t.id);
        let length = t.metadata.duration_ms.unwrap_or(16_000).min(16_000);
        // A single anchor can describe only a constant tempo. Preserve native playback
        // and a static image for a warped grid until the DJ defines a project beat.
        let uniform_grid = t.grid.filter(|g| g.is_constant_tempo());
        let bpm = match t.grid {
            Some(_) => uniform_grid.map(|g| g.bpm()),
            None => p.and_then(|p| p.bpm),
        };
        let source_bpm = bpm.filter(|b| (20.0..=400.).contains(b));
        let clip = Clip {
            path: t.path.clone(),
            title: t.display_title().into(),
            start_ms: start,
            source_ms: 0,
            length_ms: source_bpm
                .map_or(length, |b| {
                    (length as f64 * b / st.project.bpm).round() as u64
                })
                .max(1),
            source_bpm,
            first_beat_ms: uniform_grid.map(|g| g.anchor().as_millis() as u64),
            gain: vec![],
            eq: vec![],
            fader: vec![],
        };
        st.change(|p| {
            p.clips.push(clip);
            Ok(())
        })?;
        st.missing.push(!t.available);
        st.selected = Some(st.project.clips.len() - 1);
        st.point = None;
        st.notice = "Clip added. Set its project position and source excerpt.".into();
        st.refresh();
        Ok(())
    }
    fn select_timeline_clip(&self, i: i32) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        st.idle()?;
        st.selected = usize::try_from(i)
            .ok()
            .filter(|&i| i < st.project.clips.len());
        st.point = None;
        st.stop_audio();
        st.refresh();
        Ok(())
    }
    fn edit_timeline_clip(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let i = st.index()?;
        let v = st.view()?;
        let start = milliseconds(&v.get_clip_at(), "project position")?;
        let src = milliseconds(&v.get_source_at(), "source position")?;
        let len = milliseconds(&v.get_length(), "clip length")?;
        let raw = v.get_native_bpm();
        let bpm = (!raw.trim().is_empty())
            .then(|| {
                raw.trim()
                    .parse()
                    .map_err(|_| "Enter a numeric source tempo.")
            })
            .transpose()?;
        let raw = v.get_first_beat();
        let beat = (!raw.trim().is_empty())
            .then(|| milliseconds(&raw, "first beat"))
            .transpose()?;
        st.change(|p| {
            let c = &mut p.clips[i];
            c.start_ms = start;
            c.source_ms = src;
            c.length_ms = len;
            c.source_bpm = bpm;
            c.first_beat_ms = beat;
            Ok(())
        })?;
        st.notice = "Clip updated.".into();
        st.refresh();
        Ok(())
    }
    fn tempo_timeline(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let bpm = st
            .view()?
            .get_tempo()
            .trim()
            .parse()
            .map_err(|_| "Enter a numeric project tempo.")?;
        st.change(|p| {
            p.bpm = bpm;
            Ok(())
        })?;
        st.notice = "Project tempo updated.".into();
        st.refresh();
        Ok(())
    }
    fn remove_timeline_clip(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let i = st.index()?;
        st.change(|p| {
            p.clips.remove(i);
            Ok(())
        })?;
        st.missing.remove(i);
        st.selected = st
            .project
            .clips
            .len()
            .checked_sub(1)
            .map(|last| i.min(last));
        st.point = None;
        st.refresh();
        Ok(())
    }
    fn check_timeline(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let project = st.project.clone();
        st.start("Checking source files…", move |c| {
            missing_sources(&project, c).map(Output::Sources)
        })
    }
    fn relink_timeline(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let idx = st.index()?;
        st.start("Choose the matching source recording…", move |c| {
            let path = rfd::FileDialog::new()
                .add_filter("Audio", crate::audio_formats::EXTENSIONS)
                .pick_file()
                .ok_or("Relink cancelled.")?;
            c.check()?;
            crate::media_input::open_regular(&path).map_err(|e| e.to_string())?;
            Ok(Output::Relink(idx, path))
        })
    }
    fn select_timeline_curve(&self, i: i32) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        st.curve = i.clamp(0, 2);
        st.point = None;
        st.refresh();
        Ok(())
    }
    fn select_timeline_point(&self, i: i32) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let clip = st.index()?;
        st.point = usize::try_from(i)
            .ok()
            .filter(|&i| i < curve(&st.project.clips[clip], st.curve).len());
        st.refresh();
        Ok(())
    }
    fn write_timeline_point(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let i = st.index()?;
        let kind = st.curve;
        let v = st.view()?;
        let point = Point {
            ms: milliseconds(&v.get_point_at(), "automation point")?,
            value: v
                .get_point_value()
                .trim()
                .parse()
                .map_err(|_| "Enter a numeric automation value.")?,
        };
        let ms = point.ms;
        st.change(|p| {
            let points = curve_mut(&mut p.clips[i], kind);
            match points.binary_search_by_key(&point.ms, |p| p.ms) {
                Ok(idx) => points[idx] = point,
                Err(idx) => points.insert(idx, point),
            }
            Ok(())
        })?;
        st.point = curve(&st.project.clips[i], kind)
            .binary_search_by_key(&ms, |p| p.ms)
            .ok();
        st.notice = "Automation point set.".into();
        st.refresh();
        Ok(())
    }
    fn remove_timeline_point(&self) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let i = st.index()?;
        let kind = st.curve;
        let idx = st.point.ok_or("Select an automation point.")?;
        st.change(|p| {
            curve_mut(&mut p.clips[i], kind).remove(idx);
            Ok(())
        })?;
        st.point = None;
        st.refresh();
        Ok(())
    }
    fn mark_timeline_clip(&self) -> Result<(), String> {
        let st = self.timeline.borrow();
        let clip = &st.project.clips[st.index()?];
        let v = st.view()?;
        v.set_mark_in(seconds(clip.start_ms).into());
        v.set_mark_out(seconds(clip.start_ms + clip.length_ms).into());
        Ok(())
    }
    fn mark_timeline_overlap(&self) -> Result<(), String> {
        let st = self.timeline.borrow();
        let i = st.index()?;
        let clip = &st.project.clips[i];
        let mark = st
            .project
            .clips
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .filter_map(|(_, other)| {
                let a = clip.start_ms.max(other.start_ms);
                let b = (clip.start_ms + clip.length_ms).min(other.start_ms + other.length_ms);
                (a < b).then_some((a, b))
            })
            .min()
            .ok_or("This clip has no overlap. Move its project start first.")?;
        let v = st.view()?;
        v.set_mark_in(seconds(mark.0).into());
        v.set_mark_out(seconds(mark.1).into());
        Ok(())
    }
    fn render_timeline(&self, mode: i32, animate: bool) -> Result<(), String> {
        let mut st = self.timeline.borrow_mut();
        let v = st.view()?;
        let a = v.get_mark_in();
        let b = v.get_mark_out();
        let (start, end) = interval(&a, &b, st.project.end_ms())?;
        st.project.interval(start, end)?;
        let project = st.project.clone();
        let owner = st.path.clone();
        if mode == 0 {
            let e = self.engine.borrow();
            if !e.preview_route_available() || self.pending_device_switch.borrow().is_some() {
                return Err("Choose a separate headphone output first.".into());
            }
            st.start("Rendering the headphone excerpt…", move |c| {
                timeline::audition(&project, start, end.min(start + 16000), c).map(Output::Audio)
            })?;
            e.stop_tempo_preview();
            st.monitor = Some((Rc::downgrade(&e), e.preview_epoch()));
            return Ok(());
        }
        let (ext, name) = match mode {
            1 => ("wav", "WAV audio"),
            2 => ("mp4", "MP4 video"),
            _ => return Err("Unknown export format.".into()),
        };
        st.start("Rendering the marked interval…", move |c| {
            let path = destination(
                rfd::FileDialog::new()
                    .add_filter(name, &[ext])
                    .set_title(name)
                    .set_file_name(format!("set.{ext}"))
                    .save_file()
                    .ok_or("Export cancelled.")?,
                ext,
            )?;
            c.check()?;
            if owner.as_ref().is_some_and(|p| same_file(p, &path)) {
                return Err("Choose an output different from the project file.".into());
            }
            if mode == 1 {
                timeline::render_wav(&project, &path, start, end, c)?;
            } else {
                timeline::render_video(&project, &path, start, end, animate, c)?;
            }
            Ok(Output::Exported(path))
        })
    }
    pub(super) fn poll_timeline(&self) {
        let mut st = self.timeline.borrow_mut();
        if st.view.is_none() && st.job.is_none() {
            return;
        }
        let e = self.engine.borrow();
        let route = e.preview_route_available() && self.pending_device_switch.borrow().is_none();
        if st.monitor.as_ref().is_some_and(|(w, epoch)| {
            !route
                || w.upgrade()
                    .is_none_or(|old| !Rc::ptr_eq(&old, &e) || e.preview_epoch() != *epoch)
        }) {
            st.halt();
            st.notice = "Headphone output changed or another preview took over.".into();
        }
        let result = st.job.as_ref().and_then(|j| match j.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Timeline worker stopped.".into())),
        });
        if let Some(result) = result {
            let cancelled = st.job.take().is_some_and(|j| j.cancel.is_cancelled());
            let applied = (|| -> Result<String, String> {
                match result? {
                    Output::Opened(p, path, missing) if !cancelled => {
                        st.project = p;
                        st.path = Some(path);
                        st.missing = missing;
                        st.selected = (!st.project.clips.is_empty()).then_some(0);
                        st.point = None;
                        st.dirty = false;
                        if let Some(v) = &st.view {
                            v.set_mark_in("".into());
                            v.set_mark_out("".into());
                        }
                        Ok(format!(
                            "Project opened. {} missing sources.",
                            st.missing.iter().filter(|&&m| m).count()
                        ))
                    }
                    Output::Saved(path) => {
                        st.path = Some(path);
                        st.dirty = false;
                        Ok("Project saved.".into())
                    }
                    Output::Relink(i, path) if !cancelled => {
                        st.change(|p| {
                            p.clips[i].path = path;
                            Ok(())
                        })?;
                        st.missing[i] = false;
                        Ok("Source relinked. Save the project to keep this change.".into())
                    }
                    Output::Sources(m) if !cancelled => {
                        st.missing = m;
                        Ok(format!(
                            "{} missing sources.",
                            st.missing.iter().filter(|&&x| x).count()
                        ))
                    }
                    Output::Audio(clip) if !cancelled && st.monitor.is_some() && route => {
                        e.play_sample_preview(&clip)?;
                        st.monitor = Some((Rc::downgrade(&e), e.preview_epoch()));
                        Ok("Playing the marked excerpt in headphones.".into())
                    }
                    Output::Exported(path) => Ok(format!("Exported {}", path.display())),
                    _ => Ok("Operation cancelled.".into()),
                }
            })();
            st.notice = applied.unwrap_or_else(|error| error);
            st.refresh();
        }
        if let Some(v) = &st.view {
            v.set_has_route(route);
            v.set_busy(st.job.is_some());
            v.set_status(st.notice.as_str().into());
        }
    }
}
