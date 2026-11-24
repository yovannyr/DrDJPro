use super::{local_keys::Version, set_planning::Lane, *};
use crate::{cancellation::Cancellation, PassageWindow};
use drdjpro_ui_spike::{media::stems::VoiceCategory, prepared_stems::PreparedStems};
use ebur128::{EbuR128, Mode};
mod vocals;

#[derive(Clone)]
struct Pin {
    id: i32,
    path: PathBuf,
    name: String,
}
struct Comparison {
    mix: String,
    drums: String,
}
#[derive(Default)]
pub(super) struct State {
    view: Option<PassageWindow>,
    pins: [Option<Pin>; 2],
    job: Lane<Comparison>,
    vocals: vocals::VocalState,
}
impl State {
    fn invalidate(&mut self, msg: &str) {
        self.job.cancel();
        self.vocals.invalidate(self.view.as_ref());
        if let Some(v) = &self.view {
            v.set_mix("—".into());
            v.set_drums("—".into());
            v.set_status(msg.into());
        }
    }
}
fn ms(text: &str) -> Result<u64, String> {
    let v = text
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=604800.).contains(v))
        .ok_or("Enter non-negative seconds within one week.")?;
    Ok((v * 1000.).round() as u64)
}
fn loudness(m: EbuR128) -> Result<f64, String> {
    let value = m.loudness_global().map_err(|e| e.to_string())?;
    if value.is_finite() && value >= -70. {
        Ok(value)
    } else {
        Err("below the integrated loudness gate".into())
    }
}
fn full_mix(path: &Path, start: u64, len: u64, c: &Cancellation) -> Result<f64, String> {
    let mut meter = None;
    let mut offset = 0_u64;
    let mut complete = false;
    crate::playback::clip_diagnosis::stream(path, c, |ch, rate, data| {
        let rate = u64::from(rate);
        let first = start * rate / 1000;
        let end = first + len * rate / 1000;
        let next = offset + data.len() as u64 / u64::from(ch);
        let lo = offset.max(first);
        let hi = next.min(end);
        if lo < hi {
            if meter.is_none() {
                meter = Some(
                    EbuR128::new(u32::from(ch), rate as u32, Mode::I).map_err(|e| e.to_string())?,
                );
            }
            if let Some(m) = &mut meter {
                m.add_frames_f32(
                    &data[(lo - offset) as usize * usize::from(ch)
                        ..(hi - offset) as usize * usize::from(ch)],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        offset = next;
        complete = next >= end;
        Ok(complete)
    })?;
    if !complete {
        return Err("the source ends before the selected passage".into());
    }
    loudness(meter.ok_or("empty passage")?)
}
fn cached_stems(path: &Path, c: &Cancellation) -> Result<PreparedStems, String> {
    let mut source = crate::media_input::open_regular(path).map_err(|e| e.to_string())?;
    PreparedStems::cached(&mut source, || c.is_cancelled())
        .map_err(|e| format!("prepared stem unavailable ({e})"))
}
fn drums(path: &Path, start: u64, len: u64, c: &Cancellation) -> Result<f64, String> {
    let mut m = EbuR128::new(2, 44100, Mode::I).map_err(|e| e.to_string())?;
    cached_stems(path, c)?
        .visit_window(
            VoiceCategory::Drums,
            start,
            len,
            || c.is_cancelled(),
            |pcm| {
                m.add_frames_f32(pcm)?;
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    loudness(m)
}
fn difference(a: Result<f64, String>, b: Result<f64, String>) -> String {
    match (a, b) {
        (Ok(a), Ok(b)) => format!("{:+.2} LU   (A {:.2} LUFS, B {:.2} LUFS)", b - a, a, b),
        (Err(a), Err(b)) => format!("Unavailable · A: {a}; B: {b}"),
        (Err(e), _) => format!("Unavailable · A: {e}"),
        (_, Err(e)) => format!("Unavailable · B: {e}"),
    }
}
fn compare(
    paths: [PathBuf; 2],
    starts: [u64; 2],
    len: u64,
    c: &Cancellation,
) -> Result<Comparison, String> {
    let before = [Version::read(&paths[0], c)?, Version::read(&paths[1], c)?];
    let [ma, mb] = std::array::from_fn(|i| full_mix(&paths[i], starts[i], len, c));
    c.check()?;
    let [da, db] = std::array::from_fn(|i| drums(&paths[i], starts[i], len, c));
    c.check()?;
    for (i, path) in paths.iter().enumerate() {
        if Version::read(path, c)? != before[i] {
            return Err("Source changed during analysis. Measure again.".into());
        }
    }
    Ok(Comparison {
        mix: difference(ma, mb),
        drums: difference(da, db),
    })
}
impl AudioController {
    pub(super) fn open_passages(self: &Rc<Self>) -> Result<(), String> {
        if let Some(v) = &self.passages.borrow().view {
            return v.show().map_err(|e| e.to_string());
        }
        let view = PassageWindow::new().map_err(|e| e.to_string())?;
        self.wire_vocals(&view);
        let weak = Rc::downgrade(self);
        view.on_pin(move |side| {
            if let Some(c) = weak.upgrade() {
                c.pin_passage(side);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_settings_changed(move || {
            if let Some(c) = weak.upgrade() {
                c.passages
                    .borrow_mut()
                    .invalidate("Settings changed. Measure again.");
            }
        });
        let weak = Rc::downgrade(self);
        view.on_measure(move || {
            if let Some(c) = weak.upgrade() {
                c.measure_passages();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_passages();
            }
        });
        let weak = Rc::downgrade(self);
        view.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_passages();
            }
            slint::CloseRequestResponse::HideWindow
        });
        view.show().map_err(|e| e.to_string())?;
        let mut s = self.passages.borrow_mut();
        s.view = Some(view);
        s.invalidate("Pin both library tracks and select their source passages.");
        drop(s);
        if self.library.borrow().selected_id() > 0 {
            self.pin_passage(false);
        }
        Ok(())
    }
    pub(super) fn close_passages(&self) {
        let mut s = self.passages.borrow_mut();
        s.job.cancel();
        s.vocals.close();
        s.pins = [None, None];
        if let Some(v) = s.view.take() {
            let _ = v.hide();
        }
    }
    fn pin_passage(&self, b: bool) {
        let lib = self.library.borrow();
        let id = lib.selected_id();
        let mut s = self.passages.borrow_mut();
        if !self.storage_ready.get()
            || self
                .ui
                .upgrade()
                .is_none_or(|ui| ui.get_selected_track() != id)
        {
            return;
        }
        let Some(t) = lib.track_by_id(id) else {
            if let Some(v) = &s.view {
                v.set_status("Select a track in the library.".into());
            }
            return;
        };
        s.invalidate("Select the passage and measure.");
        let pin = Pin {
            id,
            path: t.path.clone(),
            name: if t.metadata.title.is_empty() {
                t.path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            } else {
                t.metadata.title.clone()
            },
        };
        if let Some(v) = &s.view {
            if b {
                v.set_title_b(pin.name.as_str().into());
            } else {
                v.set_title_a(pin.name.as_str().into());
            }
        }
        s.pins[usize::from(b)] = Some(pin);
    }
    fn measure_passages(&self) {
        let mut s = self.passages.borrow_mut();
        s.invalidate("Measuring integrated loudness…");
        let Some(v) = &s.view else {
            return;
        };
        let [Some(a), Some(b)] = &s.pins else {
            v.set_status("Pin both library tracks.".into());
            return;
        };
        let p = ms(&v.get_start_a())
            .and_then(|a| Ok(([a, ms(&v.get_start_b())?], ms(&v.get_length())?)));
        let (starts, len) = match p {
            Ok((s, l)) if (1000..=16000).contains(&l) => (s, l),
            Ok(_) => {
                v.set_status("Choose a common duration from 1 to 16 seconds.".into());
                return;
            }
            Err(e) => {
                v.set_status(e.into());
                return;
            }
        };
        let paths = [a.path.clone(), b.path.clone()];
        s.job
            .submit(Box::new(move |c| compare(paths, starts, len, c)));
    }
    pub(super) fn poll_passages(&self) {
        let mut s = self.passages.borrow_mut();
        let result = s.job.poll();
        self.poll_vocals(&mut s);
        let Some(v) = s.view.as_ref().map(ComponentHandle::clone_strong) else {
            return;
        };
        let lib = self.library.borrow();
        for idx in 0..2 {
            if s.pins[idx]
                .as_ref()
                .is_some_and(|p| lib.track_by_id(p.id).is_none_or(|t| t.path != p.path))
            {
                s.pins[idx] = None;
                if idx == 0 {
                    v.set_title_a(Default::default());
                } else {
                    v.set_title_b(Default::default());
                }
                s.invalidate("Track changed. Pin it again.");
                return;
            }
        }
        if let Some(done) = result {
            match done {
                Ok(r) => {
                    v.set_mix(r.mix.into());
                    v.set_drums(r.drums.into());
                    v.set_status(
                        "Integrated BS.1770 loudness over the selected source intervals.".into(),
                    );
                }
                Err(e) => s.invalidate(&e),
            }
        }
    }
}

#[cfg(test)]
mod loudness_qualification {
    use super::*;
    use crate::test_support::wav_samples;
    #[test]
    fn a02_equal_and_six_db_passages_and_no_drum_evidence() {
        let tone = |gain: f32| {
            let pcm: Vec<_> = (0..48_000)
                .map(|i| {
                    (gain * (i as f32 * std::f32::consts::TAU * 440.0 / 48_000.0).sin()) as i16
                })
                .collect();
            wav_samples(&pcm, 48_000, 1)
        };
        let first = tone(4000.);
        let louder = tone(8000.);
        let silent = tone(0.);
        let c = Cancellation::default();
        let a = full_mix(first.path(), 0, 1000, &c).unwrap();
        let b = full_mix(louder.path(), 0, 1000, &c).unwrap();
        assert!((b - a - 6.0206).abs() < 0.05);
        assert!(difference(Ok(a), Ok(a)).starts_with("+0.00 LU"));
        assert!(difference(Ok(b), Ok(a)).starts_with("-6.02 LU"));
        assert!(full_mix(silent.path(), 0, 1000, &c).is_err());
        assert!(full_mix(first.path(), 100, 1000, &c).is_err());
        assert!(drums(first.path(), 0, 1000, &c).is_err());
        eprintln!(
            "A02 measured passage A={a:.3} B={b:.3} LUFS, delta={:.3} LU",
            b - a
        );
    }

    pub(super) fn stem_key(file: &Path) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new_with_prefix(b"DRDJST02");
        h.update(std::fs::read(file).unwrap());
        h.finalize().into()
    }
    fn stem_fixture(dir: &Path, key: [u8; 32], gain: f32) {
        write_stem_frames(dir, key, 44_100, |i| {
            let drum = (gain * (std::f32::consts::TAU * 440.0 * i as f32 / 44_100.).sin()) as i16;
            [0, 0, drum, drum, 0, 0, 0, 0]
        });
    }
    pub(super) fn write_stem_frames(
        dir: &Path,
        key: [u8; 32],
        count: u64,
        frame: impl Fn(u64) -> [i16; 8],
    ) {
        use flate2::{write::ZlibEncoder, Compression};
        use sha2::{Digest, Sha256};
        use std::io::Write;
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        let mut buffered = std::io::BufWriter::new(&mut z);
        for i in 0..count {
            buffered
                .write_all(frame(i).map(i16::to_le_bytes).as_flattened())
                .unwrap();
        }
        buffered.flush().unwrap();
        drop(buffered);
        let payload = z.finish().unwrap();
        let mut digest = Sha256::new_with_prefix(count.to_le_bytes());
        digest.update(&payload);
        let mut file =
            std::fs::File::create(dir.join(format!("spleeter-v1-{}.stems", key[0] % 2))).unwrap();
        for part in [
            b"DRDJST02".as_slice(),
            &key,
            &count.to_le_bytes(),
            &digest.finalize(),
            &payload,
        ] {
            file.write_all(part).unwrap();
        }
    }
    #[test]
    fn a02_equal_mix_plus_six_lu_drums_and_stale_missing_or_silent_stem() {
        // The cache location is normally process-wide. Isolate this pair of synthetic
        // cache slots so concurrent tests and the DJ's own cache cannot affect the result.
        const CHILD: &str = "DRDJ_A02_QUALIFICATION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let cache = tempfile::tempdir().unwrap();
            let result=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","controller::passages::loudness_qualification::a02_equal_mix_plus_six_lu_drums_and_stale_missing_or_silent_stem","--nocapture"])
                .env(CHILD,"1").env("DRDJ_STEM_CACHE",cache.path()).output().unwrap();
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(
                result.status.success()
                    && stdout.contains("1 passed")
                    && stderr.contains("A02 stem pair"),
                "{stdout}{stderr}"
            );
            eprint!("{stderr}");
            return;
        }
        let dir = PathBuf::from(std::env::var_os("DRDJ_STEM_CACHE").unwrap());
        let mut pcm: Vec<_> = (0..48_000)
            .map(|i| (8_000.0 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.).sin()) as i16)
            .collect();
        let a = wav_samples(&pcm, 48_000, 1);
        let ka = stem_key(a.path());
        let (b, kb) = (0..128)
            .find_map(|v| {
                pcm.push(v);
                let f = wav_samples(&pcm, 48_000, 1);
                let k = stem_key(f.path());
                (k[0] % 2 != ka[0] % 2).then_some((f, k))
            })
            .unwrap();
        let c = Cancellation::default();
        let paths = [a.path().to_owned(), b.path().to_owned()];
        stem_fixture(&dir, ka, 4_000.0);
        let missing = compare(paths.clone(), [0, 0], 1000, &c).unwrap();
        assert!(missing.mix.starts_with("+0.00 LU") && missing.drums.contains("Unavailable"));
        stem_fixture(&dir, kb, 8_000.0);
        let pair = compare(paths.clone(), [0, 0], 1000, &c).unwrap();
        assert!(
            pair.mix.starts_with("+0.00 LU") && pair.drums.starts_with("+6.02 LU"),
            "{} / {}",
            pair.mix,
            pair.drums
        );
        let reverse = compare([paths[1].clone(), paths[0].clone()], [0, 0], 1000, &c).unwrap();
        assert!(reverse.drums.starts_with("-6.02 LU"));
        stem_fixture(&dir, kb, 0.);
        assert!(compare(paths.clone(), [0, 0], 1000, &c)
            .unwrap()
            .drums
            .contains("below"));
        let mut stale = kb;
        stale[1] ^= 0xff;
        stem_fixture(&dir, stale, 8_000.0);
        assert!(compare(paths, [0, 0], 1000, &c)
            .unwrap()
            .drums
            .contains("Unavailable"));
        eprintln!("A02 stem pair: {}, drums {}", pair.mix, pair.drums);
    }
}
