use super::*;
use crate::{cancellation::Cancellation, test_support::wav_samples};
use std::{fs, process::Command};

fn clip(path: &Path, title: &str, start_ms: u64, length_ms: u64) -> Clip {
    Clip {
        path: path.to_owned(),
        title: title.to_owned(),
        start_ms,
        source_ms: 0,
        length_ms,
        source_bpm: Some(120.),
        first_beat_ms: Some(0),
        gain: vec![],
        eq: vec![],
        fader: vec![],
    }
}

#[test]
fn d07_three_clips_render_sample_accurate_automation_and_preserve_old_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav_samples(&[4096; 8000], 8000, 1);
    let b = wav_samples(&[-2048; 8000], 8000, 1);
    let c = wav_samples(&[1024; 8000], 8000, 1);
    let mut ca = clip(a.path(), "first", 0, 350);
    ca.gain = vec![
        Point { ms: 0, value: 0. },
        Point {
            ms: 150,
            value: -6.0206,
        },
    ];
    let cb = clip(b.path(), "second", 200, 370);
    let mut cc = clip(c.path(), "third", 470, 280);
    cc.eq = vec![Point {
        ms: 0,
        value: 6.0206,
    }];
    let p = Project {
        version: 1,
        bpm: 120.,
        clips: vec![ca, cb, cc],
    };
    let project = dir.path().join("set.drdjset");
    p.save(&project).unwrap();
    let cancel = Cancellation::default();
    let reopened = Project::load(&project, &cancel).unwrap();
    let out = dir.path().join("set.wav");
    render_wav(&p, &out, 0, 750, &cancel).unwrap();
    let first = fs::read(&out).unwrap();
    assert_eq!(first.len(), 44 + 750 * 48 * 4);
    let left = |frame: usize| {
        i16::from_le_bytes(first[44 + frame * 4..46 + frame * 4].try_into().unwrap())
    };
    for (frame, want) in [
        (0, 4096),
        (9599, 2048),
        (9600, 0),
        (16799, 0),
        (16800, -2048),
        (25920, 0),
        (27360, 2048),
        (35999, 2048),
    ] {
        assert!(
            (i32::from(left(frame)) - want).abs() <= 2,
            "sample {frame} got {}",
            left(frame)
        );
    }
    render_wav(&reopened, &out, 0, 750, &cancel).unwrap();
    assert_eq!(fs::read(&out).unwrap(), first);
    let mut blocks = 0;
    let exhausted = super::audio::render_wav_to(&p, &out, 0, 750, &cancel, |writer, buf| {
        blocks += 1;
        if blocks == 3 {
            Err(std::io::ErrorKind::StorageFull.into())
        } else {
            writer.write_all(buf)
        }
    })
    .unwrap_err();
    assert_eq!(blocks, 3);
    assert_eq!(
        exhausted,
        std::io::Error::from(std::io::ErrorKind::StorageFull).to_string()
    );
    assert_eq!(fs::read(&out).unwrap(), first);
    let mut missing = reopened.clone();
    missing.clips[1].path = dir.path().join("missing.wav");
    assert!(render_wav(&missing, &out, 0, 750, &cancel).is_err());
    assert_eq!(fs::read(&out).unwrap(), first);
    cancel.request();
    assert!(render_wav(&p, &out, 0, 750, &cancel).is_err());
    assert_eq!(fs::read(&out).unwrap(), first);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    println!(
        "D07: 36,000 stereo frames, {} bytes, three clips, two transitions",
        first.len()
    );
}

#[test]
#[ignore = "requires local ffmpeg and ffprobe with H.264/AAC"]
fn r07_reopens_30s_video_and_measures_title_and_beat_frames() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav_samples(&vec![2048; 8_000 * 12], 8000, 1);
    let b = wav_samples(&vec![-2048; 8_000 * 18], 8000, 1);
    let p = Project {
        version: 1,
        bpm: 120.,
        clips: vec![
            clip(a.path(), "AAAAA", 0, 12_000),
            clip(b.path(), "WWWWWWWWWWWW", 12_000, 18_000),
        ],
    };
    let out = dir.path().join("test.mp4");
    let cancel = Cancellation::default();
    render_video(&p, &out, 0, 30_000, true, &cancel).unwrap();
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = meta["streams"].as_array().unwrap();
    let video = streams.iter().find(|v| v["codec_type"] == "video").unwrap();
    let audio = streams.iter().find(|v| v["codec_type"] == "audio").unwrap();
    assert_eq!(video["avg_frame_rate"], "30/1");
    assert_eq!(video["nb_frames"], "900");
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(audio["codec_name"], "aac");
    assert_eq!(audio["sample_rate"], "48000");
    for stream in [audio, video] {
        let seconds = stream["duration"].as_str().unwrap().parse::<f64>().unwrap();
        assert!((seconds - 30.).abs() <= 1. / 30.);
    }
    // Decode five full-resolution frames to gray. Comparing restricted regions avoids false
    // positives from the animated beat bar and tolerates small H.264 quantisation differences.
    let frames = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "1", "-i"])
        .arg(&out)
        .args([
            "-an",
            "-vf",
            r"select=eq(n\,0)+eq(n\,359)+eq(n\,360)+eq(n\,884)+eq(n\,885)",
            "-fps_mode",
            "vfr",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        frames.status.success(),
        "{}",
        String::from_utf8_lossy(&frames.stderr)
    );
    let size = 640 * 360;
    assert_eq!(frames.stdout.len(), 5 * size);
    let pixel = |frame: usize, x: usize, y: usize| frames.stdout[frame * size + y * 640 + x];
    for frame in [0, 2, 4] {
        assert!(
            pixel(frame, 100, 341) > 140,
            "missing beat in frame index {frame}"
        );
    }
    for frame in [1, 3] {
        assert!(pixel(frame, 100, 341) < 85);
    }
    let title_difference = |a, b| {
        (297..331)
            .map(|y| {
                (24..430)
                    .filter(|&x| pixel(a, x, y).abs_diff(pixel(b, x, y)) > 60)
                    .count()
            })
            .sum::<usize>()
    };
    assert!(title_difference(0, 1) < 50);
    assert!(title_difference(1, 2) > 100);
    assert!(title_difference(2, 4) < 50);
    assert!(pixel(0, 320, 131) > pixel(0, 220, 30) + 30);
    // Find the polarity reversal in independently decoded audio, including its stream PTS.
    // One millisecond bins reject individual AAC ringing samples at the cut.
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "1", "-i"])
        .arg(&out)
        .args([
            "-map", "0:a:0", "-vn", "-ac", "1", "-ar", "8000", "-t", "30", "-f", "s16le", "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    let means: Vec<_> = decoded
        .stdout
        .as_chunks::<16>()
        .0
        .iter()
        .map(|bytes| {
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i32::from(i16::from_le_bytes(*b)))
                .sum::<i32>()
                / 8
        })
        .collect();
    assert!(means.len() >= 29_000);
    let transition_ms = means
        .windows(4)
        .position(|window| window.iter().all(|&sample| sample < -768))
        .expect("negative audio plateau");
    assert!(transition_ms >= 100 && means[transition_ms - 100] > 768);
    assert!(means[means.len() - 100] < -768);
    let start_seconds = |stream: &serde_json::Value| {
        stream["start_time"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    let audio_frame =
        (start_seconds(audio) - start_seconds(video) + transition_ms as f64 / 1000.) * 30.;
    let offset_frames = audio_frame - 360.;
    println!("R07 AAC transition at {transition_ms} ms, video title at frame 360, A/V offset {offset_frames:+.3} frames");
    assert!(
        offset_frames.abs() <= 1.,
        "A/V offset {offset_frames} frames"
    );
    let complete = fs::read(&out).unwrap();
    // Wait for the encoder to put a header in its staging file. Cancelling here
    // exercises the active pipe, child process and both temporary artifacts.
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| render_video(&p, &out, 0, 30_000, false, &cancel));
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(45);
        let mut encoding = false;
        while !worker.is_finished() && std::time::Instant::now() < limit {
            encoding = fs::read_dir(dir.path()).unwrap().flatten().any(|f| {
                let path = f.path();
                path != out
                    && path.extension().is_some_and(|ext| ext == "mp4")
                    && f.metadata().is_ok_and(|m| m.len() > 0)
            });
            if encoding {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cancel.request();
        assert!(worker.join().unwrap().is_err());
        assert!(encoding, "expected to interrupt an active encoder");
    });
    assert_eq!(fs::read(&out).unwrap(), complete);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    println!("R07: 900 frames at 30 fps; title changes at frame 360, beats at 0/360/885; H.264/AAC, {} bytes",complete.len());
}
