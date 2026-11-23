use super::*;
use crate::{
    cancellation::Cancellation,
    playback::clip_diagnosis::{comparison, scan, stream_processed},
    test_support::{wav_samples, TempFile},
};

const RATE: u32 = 24_000;
const START: usize = RATE as usize * 9;
const END: usize = RATE as usize * 11;
const LENGTH: usize = RATE as usize * 14;

fn signal(i: usize, clipped: bool) -> i16 {
    let phase = std::f64::consts::TAU * (i % 24) as f64 / 24.;
    let amplitude = if clipped && (START..END).contains(&i) {
        1.2
    } else {
        0.70
    };
    ((phase.sin() * amplitude).clamp(-0.70, 0.70) * 32767.).round() as i16
}
fn fixture() -> TempFile {
    // Only the right channel clips. Both channels have exactly the same peak outside the defect.
    let pcm = (0..LENGTH)
        .flat_map(|i| [signal(i, false), signal(i, true)])
        .collect::<Vec<_>>();
    wav_samples(&pcm, RATE, 2)
}

#[test]
fn a01_scan_repeated_plateaus_after_eight_seconds_but_not_single_peaks_sines_or_silence() {
    let file = fixture();
    let cancel = Cancellation::default();
    let report = scan(file.path(), &cancel).unwrap();
    assert_eq!(report.duration, Duration::from_secs(14));
    assert_eq!(report.markers.len(), 1);
    let marker = report.markers[0];
    assert!((marker.start.as_secs_f64() - 9.).abs() < 0.002);
    assert!((marker.end.as_secs_f64() - 11.).abs() < 0.002);
    let clean = (0..RATE as usize * 2)
        .map(|i| signal(i, false))
        .collect::<Vec<_>>();
    let mut isolated = vec![0; clean.len()];
    isolated[135..140].fill(32767);
    for pcm in [&clean, &isolated, &vec![0; clean.len()]] {
        let negative = wav_samples(pcm, RATE, 1);
        assert!(scan(negative.path(), &cancel).unwrap().markers.is_empty());
    }
    let opus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/format-opus.opus");
    assert!(scan(&opus, &cancel).unwrap().duration > Duration::ZERO);
}

#[test]
fn a01_comparison_and_export_align_and_preserve_source_with_two_decks() {
    use sha2::{Digest, Sha256};
    let file = fixture();
    let checksum = || Sha256::digest(std::fs::read(file.path()).unwrap());
    let before = checksum();
    let cancel = Cancellation::default();
    let report = scan(file.path(), &cancel).unwrap();
    assert!(comparison(file.path(), &report, 1, &cancel).is_err());
    let (raw, edited) = comparison(file.path(), &report, 0, &cancel).unwrap();
    assert_eq!(
        (raw.rate, raw.channels, raw.start),
        (edited.rate, edited.channels, edited.start)
    );
    assert_eq!(raw.pcm.len(), RATE as usize * 8 * 2);
    assert_eq!(raw.pcm.len(), edited.pcm.len());
    assert!(raw
        .pcm
        .iter()
        .chain(edited.pcm.iter())
        .all(|v| v.is_finite() && v.abs() <= 0.981));

    let mut export = Vec::new();
    stream_processed(file.path(), &report, &cancel, |ch, rate, pcm| {
        assert_eq!((ch, rate), (2, RATE));
        export.extend_from_slice(pcm);
        Ok(())
    })
    .unwrap();
    assert_eq!(export.len(), LENGTH * 2);
    for i in (0..START).chain(END..LENGTH) {
        assert_eq!(export[2 * i + 1], f32::from(signal(i, true)) / 32768.);
    }
    let offset = frame_at(raw.start, RATE) * 2;
    let at = 2 * 3 + 1;
    let gain_a = raw.pcm[at] / export[offset + at];
    let gain_b = edited.pcm[at] / export[offset + at];
    assert!(gain_a > 0. && gain_b > 0.);
    let mut changed = 0;
    for i in 0..raw.pcm.len() / 2 {
        let t = offset / 2 + i;
        let source = f32::from(signal(t, true)) / 32768.;
        let processed = export[2 * t + 1];
        assert!((raw.pcm[2 * i + 1] - source * gain_a).abs() < 1e-5);
        assert!((edited.pcm[2 * i + 1] - processed * gain_b).abs() < 1e-5);
        changed += usize::from((source - processed).abs() > 0.02);
    }
    assert!(changed > RATE as usize / 2);

    let deck = wav_file(4096, 48_000);
    let make = || {
        let (mixer, output) = rodio::mixer::mixer(4, 48_000);
        let engine = DualDeckEngine::from_mixer(mixer, 4, 48_000);
        for id in [DeckId::A, DeckId::B] {
            engine
                .try_install(id, PreparedTrack::open(deck.path()).unwrap())
                .unwrap();
            engine.play(id);
        }
        (engine, output)
    };
    let (engine, mut output) = make();
    let (_control, mut baseline) = make();
    for _ in 0..480 * 4 {
        assert_eq!(output.next(), baseline.next());
    }
    for (clip, playing) in [(Some(&raw), true), (Some(&edited), true), (None, false)] {
        if let Some(clip) = clip {
            engine.play_sample_preview(clip).unwrap();
        } else {
            engine.stop_tempo_preview();
        }
        let mut heard = false;
        let measured = crate::test_alloc::measured_heap(|| {
            for _ in 0..480 {
                let a: [f32; 4] = std::array::from_fn(|_| output.next().unwrap());
                let b: [f32; 4] = std::array::from_fn(|_| baseline.next().unwrap());
                assert_eq!(a[..2], b[..2]);
                assert_eq!(b[2..], [0., 0.]);
                heard |= a[2].abs() > 0.01;
            }
        });
        assert_eq!(heard, playing);
        assert_eq!(
            (
                measured.allocations,
                measured.reallocations,
                measured.deallocations
            ),
            (0, 0, 0)
        );
    }
    assert_eq!(before, checksum());
}

#[test]
fn a01_cancel_and_changed_source_invalidate_scan_comparison_and_export() {
    use std::io::{Seek, SeekFrom, Write};
    let file = fixture();
    let cancel = Cancellation::default();
    let report = scan(file.path(), &cancel).unwrap();
    let mut blocks = 0;
    let stopped = stream_processed(file.path(), &report, &cancel, |_, _, _| {
        blocks += 1;
        cancel.request();
        Ok(())
    });
    assert!(stopped.is_err());
    assert_eq!(blocks, 1);
    assert!(scan(file.path(), &cancel).is_err());
    assert!(comparison(file.path(), &report, 0, &cancel).is_err());
    let cancel = Cancellation::default();
    let mut changed = false;
    let result = stream_processed(file.path(), &report, &cancel, |_, _, _| {
        if !changed {
            let mut input = std::fs::OpenOptions::new()
                .write(true)
                .open(file.path())
                .unwrap();
            input.seek(SeekFrom::Start(44)).unwrap();
            input.write_all(&[0x23, 0x45]).unwrap();
            changed = true;
        }
        Ok(())
    });
    assert!(changed && result.is_err());
    std::fs::write(file.path(), b"changed").unwrap();
    assert!(comparison(file.path(), &report, 0, &cancel).is_err());
    assert!(
        stream_processed(file.path(), &report, &cancel, |_, _, _| panic!("stale PCM")).is_err()
    );
}

#[test]
#[ignore = "independent FFmpeg EBU R128 measurement of both monitoring signals"]
fn a01_independent_ffmpeg_integrated_loudness() {
    use std::{
        fs::File,
        io::{BufWriter, Write},
        process::Command,
    };
    let file = fixture();
    let cancel = Cancellation::default();
    let report = scan(file.path(), &cancel).unwrap();
    let (a, b) = comparison(file.path(), &report, 0, &cancel).unwrap();
    let measure = |clip: &PreviewClip| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("monitor.f32le");
        let mut writer = BufWriter::new(File::create(&path).unwrap());
        for &sample in clip.pcm.iter() {
            writer.write_all(&sample.to_le_bytes()).unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        let result = Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-nostats", "-f", "f32le", "-ar"])
            .arg(clip.rate.to_string())
            .arg("-ac")
            .arg(clip.channels.to_string())
            .arg("-i")
            .arg(path)
            .args(["-af", "ebur128=framelog=verbose", "-f", "null", "-"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{log}");
        log.lines()
            .rev()
            .find_map(|line| line.trim_start().strip_prefix("I:"))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok())
            .expect("integrated loudness from FFmpeg")
    };
    let (a, b) = (measure(&a), measure(&b));
    eprintln!("A01 independent FFmpeg integrated loudness original={a:.1} LUFS processed={b:.1} LUFS difference={:.1} LU",(a-b).abs());
    assert!(a.is_finite() && b.is_finite() && (a - b).abs() <= 0.5);
}

#[test]
#[ignore = "FFmpeg LAME encode and independent gapless MP3 decode"]
fn a01_gapless_mp3_matches_independent_decoded_frame_count() {
    compare_gapless_frames("mp3", &["-c:a", "libmp3lame", "-write_xing", "1"]);
}

#[test]
#[ignore = "FFmpeg AAC encode and independent M4A edit-list decode"]
fn a01_gapless_aac_matches_independent_decoded_frame_count() {
    compare_gapless_frames("m4a", &["-c:a", "aac"]);
}

fn compare_gapless_frames(ext: &str, encoder: &[&str]) {
    use std::process::Command;
    const FRAMES: usize = 48_137;
    let samples: Vec<_> = (0..FRAMES).map(|i| signal(i, false)).collect();
    let wav = wav_samples(&samples, RATE, 1);
    let dir = tempfile::tempdir().unwrap();
    let encoded_path = dir.path().join(format!("gapless.{ext}"));
    let encoded = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-i"])
        .arg(wav.path())
        .args(encoder)
        .arg(&encoded_path)
        .output()
        .unwrap();
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    let reference = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-i"])
        .arg(&encoded_path)
        .args(["-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(
        reference.status.success(),
        "{}",
        String::from_utf8_lossy(&reference.stderr)
    );
    let independent = reference.stdout.len() / 4;
    assert_eq!(independent, FRAMES);
    let cancel = Cancellation::default();
    let report = scan(&encoded_path, &cancel).unwrap();
    let mut exported = 0;
    stream_processed(&encoded_path, &report, &cancel, |ch, rate, pcm| {
        assert_eq!((ch, rate), (1, RATE));
        exported += pcm.len();
        Ok(())
    })
    .unwrap();
    eprintln!(
        "A01 gapless {ext} original={FRAMES} FFmpeg={independent} scan={} export={exported} frames",
        frame_at(report.duration, RATE)
    );
    assert_eq!(
        (frame_at(report.duration, RATE), exported),
        (independent, independent)
    );
}

#[test]
#[ignore = "two 30 s two-deck measurements with whole-track scans and alternating A/B auditions"]
fn a01_equal_duration_clip_check_with_two_decks() {
    use std::sync::mpsc;
    let file = fixture();
    let path = file.path();
    for active in [false, true] {
        eprintln!("A01 clip scan and loudness-matched PFL active={active}");
        let (audio, switches) = std::thread::scope(|scope| {
            let (send, receive) =
                mpsc::sync_channel::<Arc<crate::playback::tempo_preview::PreviewControl>>(1);
            let (finish, stop) = mpsc::channel::<()>();
            let worker = scope.spawn(move || {
                let Ok(control) = receive.recv() else {
                    return 0;
                };
                let cancel = Cancellation::default();
                let mut switches = 0usize;
                while stop.recv_timeout(Duration::from_millis(500))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    let report = scan(path, &cancel).unwrap();
                    let (a, b) = comparison(path, &report, 0, &cancel).unwrap();
                    let clip = if switches.is_multiple_of(2) { &a } else { &b };
                    control.start_sample(
                        crate::playback::tempo_preview::Excerpt::new(clip, 48_000).unwrap(),
                    );
                    switches += 1;
                }
                switches
            });
            let audio = super::render_bench::measure_routed(
                30,
                4,
                move |engine| {
                    if active {
                        let cancel = Cancellation::default();
                        let report = scan(path, &cancel).unwrap();
                        let (first, _) = comparison(path, &report, 0, &cancel).unwrap();
                        engine.play_sample_preview(&first).unwrap();
                        send.send(engine.tempo_preview.clone()).unwrap();
                    }
                },
                || {},
                Some(active),
            );
            drop(finish);
            (audio, worker.join().unwrap())
        });
        eprintln!("A01 whole-track scans and A/B switches={switches}");
        assert!(if active {
            switches >= 10
        } else {
            switches == 0
        });
        assert_eq!(
            audio,
            (0, 0, 0, 0),
            "overruns, deadline misses, signal errors, render allocations"
        );
    }
}
