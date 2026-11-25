use super::*;
use crate::{chord_timing::PairTiming, playback::PreviewClip};
use std::time::Instant;

type Range = (u64, u64);
struct Activity {
    ranges: Vec<Range>,
    uncertain: bool,
}
struct Overlaps {
    ranges: Vec<Range>,
    uncertain: bool,
    timing: PairTiming,
    versions: [Version; 2],
}

// Isolated vocals must exceed -45 dBFS RMS and carry at least 8% as much
// power as the three other isolated stems. Weaker energy is labelled uncertain.
fn activity(path: &Path, t: PairTiming, side: usize, c: &Cancellation) -> Result<Activity, String> {
    let stems = cached_stems(path, c)?;
    let Some((start, end)) = t.source_window(side) else {
        return Ok(Activity {
            ranges: vec![],
            uncertain: false,
        });
    };
    let bins = stems
        .vocal_evidence(start, end - start, || c.is_cancelled())
        .map_err(|e| e.to_string())?;
    Ok(bin_activity(&bins, start, end, t, side))
}

fn bin_activity(bins: &[(f64, f64)], start: u64, end: u64, t: PairTiming, side: usize) -> Activity {
    let mut ranges = Vec::new();
    let mut uncertain = false;
    let mut from = None;
    for (i, value) in bins
        .iter()
        .map(Some)
        .chain(std::iter::once(None))
        .enumerate()
    {
        let at = (start + i as u64 * 50).min(end);
        let active = value.is_some_and(|&(v, m)| v >= 0.000031623 && v >= m * 0.08);
        if active {
            from.get_or_insert(at);
        } else {
            if value.is_some_and(|&(v, _)| v >= 0.000001) {
                uncertain = true;
            }
            if let Some(first) = from.take() {
                if at - first >= 100 {
                    let mix = |ms| t.mix_ms(side, ms).round().clamp(0., t.length_ms as f64) as u64;
                    let (a, b) = (mix(first), mix(at));
                    if a < b {
                        ranges.push((a, b));
                    }
                } else {
                    uncertain = true;
                }
            }
        }
    }
    Activity { ranges, uncertain }
}
fn intersection(a: &[Range], b: &[Range]) -> Vec<Range> {
    let (mut i, mut j) = (0, 0);
    let mut found = Vec::new();
    while let (Some(&(a0, a1)), Some(&(b0, b1))) = (a.get(i), b.get(j)) {
        let (lo, hi) = (a0.max(b0), a1.min(b1));
        if lo < hi {
            found.push((lo, hi));
        }
        if a1 <= b1 {
            i += 1;
        }
        if b1 <= a1 {
            j += 1;
        }
    }
    found
}
fn inspect(paths: [PathBuf; 2], timing: PairTiming, c: &Cancellation) -> Result<Overlaps, String> {
    let versions = [Version::read(&paths[0], c)?, Version::read(&paths[1], c)?];
    let a = activity(&paths[0], timing, 0, c).map_err(|e| format!("A · {e}"))?;
    let b = activity(&paths[1], timing, 1, c).map_err(|e| format!("B · {e}"))?;
    verify(&paths, &versions, c)?;
    Ok(Overlaps {
        ranges: intersection(&a.ranges, &b.ranges),
        uncertain: a.uncertain || b.uncertain,
        timing,
        versions,
    })
}
fn verify(paths: &[PathBuf; 2], versions: &[Version; 2], c: &Cancellation) -> Result<(), String> {
    for (path, v) in paths.iter().zip(versions) {
        if &Version::read(path, c)? != v {
            return Err("Source changed. Recalculate vocal activity.".into());
        }
    }
    Ok(())
}
#[derive(Default)]
pub(super) struct VocalState {
    scan: Lane<Overlaps>,
    audio: Lane<PreviewClip>,
    found: Option<Arc<Overlaps>>,
    selected: Option<usize>,
    follow: bool,
    due: Option<Instant>,
    owner: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
}
impl VocalState {
    fn stop(&mut self) {
        self.audio.cancel();
        if let Some((device, epoch)) = self.owner.take() {
            if let Some(e) = device.upgrade().filter(|e| e.preview_epoch() == epoch) {
                e.stop_tempo_preview();
            }
        }
    }
    pub(super) fn close(&mut self) {
        self.stop();
        self.scan.cancel();
        self.found = None;
        self.selected = None;
        self.follow = false;
        self.due = None;
    }
    pub(super) fn invalidate(&mut self, v: Option<&PassageWindow>) {
        self.stop();
        self.scan.cancel();
        self.found = None;
        self.selected = None;
        self.due = self
            .follow
            .then(|| Instant::now() + Duration::from_millis(400));
        if let Some(v) = v {
            v.set_overlap_index(-1);
            v.set_overlaps(Default::default());
            v.set_vocal_status(if self.follow {
                "Updating vocal alignment…".into()
            } else {
                Default::default()
            });
        }
    }
}
fn timing(v: &PassageWindow) -> Result<PairTiming, String> {
    let tempo = |x: &str| {
        x.trim()
            .parse::<f64>()
            .map_err(|_| "Enter a tempo multiplier between 0.5 and 2.")
    };
    let delay = v
        .get_offset_b()
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() <= 16.)
        .ok_or("Enter B delay within the transition window.")?;
    let t = PairTiming {
        starts_ms: [ms(&v.get_start_a())?, ms(&v.get_start_b())?],
        length_ms: ms(&v.get_length())?,
        offset_b_ms: (delay * 1000.).round() as i64,
        tempos: [tempo(&v.get_tempo_a())?, tempo(&v.get_tempo_b())?],
        pitches: [0, 0],
    };
    if t.length_ms < 1000 {
        return Err("Choose at least one second.".into());
    }
    t.validate()?;
    Ok(t)
}
impl AudioController {
    pub(super) fn wire_vocals(self: &Rc<Self>, v: &PassageWindow) {
        let w = Rc::downgrade(self);
        v.on_scan_vocals(move || {
            if let Some(c) = w.upgrade() {
                let mut s = c.passages.borrow_mut();
                s.vocals.follow = true;
                s.vocals.due = Some(Instant::now());
                drop(s);
                c.poll_passages();
            }
        });
        let w = Rc::downgrade(self);
        v.on_select_overlap(move |i| {
            if let Some(c) = w.upgrade() {
                let mut s = c.passages.borrow_mut();
                s.vocals.stop();
                s.vocals.selected = usize::try_from(i)
                    .ok()
                    .filter(|i| s.vocals.found.as_ref().is_some_and(|f| *i < f.ranges.len()));
                if let Some(v) = &s.view {
                    v.set_overlap_index(s.vocals.selected.map_or(-1, |i| i as i32));
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_stop_overlap(move || {
            if let Some(c) = w.upgrade() {
                c.passages.borrow_mut().vocals.stop();
            }
        });
        let w = Rc::downgrade(self);
        v.on_hear_overlap(move || {
            if let Some(c) = w.upgrade() {
                c.hear_overlap();
            }
        });
    }
    fn hear_overlap(&self) {
        let mut s = self.passages.borrow_mut();
        s.vocals.stop();
        let Some(found) = s.vocals.found.clone() else {
            return;
        };
        let Some(&(start, end)) = s.vocals.selected.and_then(|i| found.ranges.get(i)) else {
            return;
        };
        let [Some(a), Some(b)] = &s.pins else {
            return;
        };
        let paths = [a.path.clone(), b.path.clone()];
        let original = found.timing;
        let source_start = |i: usize| {
            let offset = if i == 1 { original.offset_b_ms } else { 0 };
            (original.starts_ms[i] as f64 + (start as f64 - offset as f64) * original.tempos[i])
                .round()
                .max(0.) as u64
        };
        let adjusted = PairTiming {
            starts_ms: [source_start(0), source_start(1)],
            length_ms: end - start,
            offset_b_ms: 0,
            ..original
        };
        let engine = self.engine.borrow();
        if !engine.preview_route_available() || self.pending_device_switch.borrow().is_some() {
            return;
        }
        engine.stop_tempo_preview();
        s.vocals.owner = Some((Rc::downgrade(&engine), engine.preview_epoch()));
        s.vocals.audio.submit(Box::new(move |c| {
            verify(&paths, &found.versions, c)?;
            for p in &paths {
                let _available = cached_stems(p, c)?;
            }
            let clip = crate::playback::chord_preview::render(&paths[0], &paths[1], adjusted, c)?;
            verify(&paths, &found.versions, c)?;
            Ok(clip)
        }));
    }
    pub(super) fn poll_vocals(&self, s: &mut State) {
        let Some(v) = s.view.as_ref() else {
            let _ = s.vocals.audio.poll();
            let _ = s.vocals.scan.poll();
            return;
        };
        if s.vocals.due.is_some_and(|d| d <= Instant::now()) {
            s.vocals.due = None;
            s.vocals.stop();
            s.vocals.scan.cancel();
            s.vocals.found = None;
            s.vocals.selected = None;
            v.set_overlaps(Default::default());
            v.set_overlap_index(-1);
            if let [Some(a), Some(b)] = &s.pins {
                match timing(v) {
                    Ok(t) => {
                        let paths = [a.path.clone(), b.path.clone()];
                        s.vocals
                            .scan
                            .submit(Box::new(move |c| inspect(paths, t, c)));
                        v.set_vocal_status("Checking prepared vocal stems…".into());
                    }
                    Err(e) => v.set_vocal_status(e.into()),
                }
            } else {
                v.set_vocal_status("Pin both tracks to align their vocals.".into());
            }
        }
        if let Some(r) = s.vocals.scan.poll() {
            match r {
                Ok(f) => {
                    let status = match (f.ranges.is_empty(), f.uncertain) {
                        (true, false) => "No overlapping vocals in this window.",
                        (true, true) => "No confident overlap. Some vocal activity is uncertain.",
                        (false, true) => {
                            "Confirmed overlaps shown. Other vocal activity is uncertain."
                        }
                        (false, false) => "Vocal overlaps on the shared transition timeline.",
                    };
                    v.set_overlaps(ModelRc::new(VecModel::from(
                        f.ranges
                            .iter()
                            .map(|(a, b)| {
                                slint::SharedString::from(format!(
                                    "{:.3} – {:.3} s",
                                    *a as f64 / 1000.,
                                    *b as f64 / 1000.
                                ))
                            })
                            .collect::<Vec<_>>(),
                    )));
                    s.vocals.selected = (!f.ranges.is_empty()).then_some(0);
                    v.set_overlap_index(s.vocals.selected.map_or(-1, |i| i as i32));
                    s.vocals.found = Some(Arc::new(f));
                    v.set_vocal_status(status.into());
                }
                Err(e) => v.set_vocal_status(e.into()),
            }
        }
        let engine = self.engine.borrow();
        let route =
            engine.preview_route_available() && self.pending_device_switch.borrow().is_none();
        v.set_vocal_route(route);
        if s.vocals.owner.as_ref().is_some_and(|(weak, epoch)| {
            !route
                || weak
                    .upgrade()
                    .is_none_or(|e| !Rc::ptr_eq(&e, &engine) || e.preview_epoch() != *epoch)
        }) {
            s.vocals.stop();
        }
        if let (Some(audio), Some((_, epoch))) = (s.vocals.audio.poll(), s.vocals.owner.as_mut()) {
            match audio.and_then(|p| engine.play_sample_preview(&p)) {
                Ok(()) => {
                    *epoch = engine.preview_epoch();
                    v.set_vocal_status("Playing selected overlap on headphones.".into());
                }
                Err(e) => {
                    s.vocals.owner = None;
                    v.set_vocal_status(e.into());
                }
            }
        }
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    include!("vocal_routing_tests.rs");

    #[test]
    fn r08_vocal_overlap_tracks_silence_offset_and_weak_stems() {
        let timing = PairTiming {
            starts_ms: [0, 0],
            length_ms: 10_000,
            offset_b_ms: 0,
            tempos: [1., 1.],
            pitches: [0, 0],
        };
        let bins = |a, b| {
            (0..200)
                .map(|i| (if (a..b).contains(&(i * 50)) { 0.15 } else { 0. }, 0.1))
                .collect::<Vec<_>>()
        };
        let a = bin_activity(&bins(4000, 8000), 0, 10000, timing, 0);
        let b = bin_activity(&bins(6000, 10000), 0, 10000, timing, 1);
        assert_eq!(intersection(&a.ranges, &b.ranges), [(6000, 8000)]);
        let late = bin_activity(
            &bins(6000, 10000),
            0,
            10000,
            PairTiming {
                offset_b_ms: 2000,
                ..timing
            },
            1,
        );
        assert!(intersection(&a.ranges, &late.ranges).is_empty());
        let silent = bin_activity(&[(0., 0.1); 200], 0, 10000, timing, 0);
        assert!(silent.ranges.is_empty() && !silent.uncertain);
        let weak = bin_activity(&[(0.000002, 0.1); 200], 0, 10000, timing, 0);
        assert!(weak.ranges.is_empty() && weak.uncertain);
    }
}
