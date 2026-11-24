use crate::{
    cancellation::Cancellation,
    midi_file, midi_scene, scene_bridge, scene_transcription,
    set_timeline::{render_video, Clip, Project},
    test_support::wav_samples,
};
use std::{
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

fn prepare_scene(source: &Path, destination: &Path) {
    let timing = midi_scene::Scene::default().timing;
    let cancel = Cancellation::default();
    let notes: Vec<_> =
        scene_transcription::transcribe(source, 0, 8_000, timing.bpm, timing.ppq, &cancel)
            .unwrap()
            .into_iter()
            .filter_map(|(note, uncertain)| (!uncertain).then_some(note))
            .collect();
    assert!(notes.len() >= 4, "expected a pitched phrase");
    let [bass, chords] = scene_bridge::variants(&notes, timing, 9, true).unwrap();
    assert!(!bass.is_empty() && !chords.is_empty() && bass != chords);
    midi_file::write(destination, timing, &bass, &cancel).unwrap();
}

fn work_on_request(rx: mpsc::Receiver<()>, completed: &AtomicUsize, work: impl Fn()) {
    for () in rx {
        work();
        completed.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
#[ignore = "paired 30 s audio run with real H.264/AAC FFmpeg export and MIDI preparation; needs ffmpeg and ffprobe"]
fn r07_m03_equal_duration_video_and_midi_scene_preparation() {
    let dir = tempfile::tempdir().unwrap();
    let rate = 8_000;
    let source = {
        let pcm = (0..30 * rate)
            .map(|i| {
                let note = i / (rate / 2) % 4;
                let frame = i % (rate / 2);
                let hz = [220.0, 261.626, 329.628, 391.995][note];
                if frame < 3 * rate / 8 {
                    (10_000.0 * (std::f32::consts::TAU * hz * frame as f32 / rate as f32).sin())
                        as i16
                } else {
                    0
                }
            })
            .collect::<Vec<_>>();
        wav_samples(&pcm, rate as u32, 1)
    };
    let source_path = source.path();
    let project = Project {
        version: 1,
        bpm: 120.,
        clips: vec![Clip {
            path: source_path.to_owned(),
            title: "Scene render".into(),
            start_ms: 0,
            source_ms: 0,
            length_ms: 30_000,
            source_bpm: Some(120.),
            first_beat_ms: Some(0),
            gain: vec![],
            eq: vec![],
            fader: vec![],
        }],
    };
    let mp4 = dir.path().join("clip.mp4");
    let midi = dir.path().join("scene.mid");
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        let video_count = AtomicUsize::new(0);
        let scene_count = AtomicUsize::new(0);
        let video_job =
            || render_video(&project, &mp4, 0, 30_000, true, &Cancellation::default()).unwrap();
        let midi_job = || prepare_scene(source_path, &midi);
        let mut observed = [0; 2];
        eprintln!("R07/M03/M04 simultaneous 30 s H.264 export and four-bar MIDI preparation active={active}");
        let stats = std::thread::scope(|scope| {
            let (video, video_rx) = mpsc::sync_channel(0);
            let (scene, scene_rx) = mpsc::sync_channel(0);
            let completed_videos = &video_count;
            let completed_scenes = &scene_count;
            scope.spawn(move || work_on_request(video_rx, completed_videos, video_job));
            scope.spawn(move || work_on_request(scene_rx, completed_scenes, midi_job));
            let audio = super::render_bench::measure_routed(
                30,
                4,
                |_| {},
                || {
                    // This hook runs within the measured interval, once per second. A busy
                    // worker skips a request; background I/O cannot queue or block the render thread.
                    observed = [
                        video_count.load(Ordering::Relaxed),
                        scene_count.load(Ordering::Relaxed),
                    ];
                    if active {
                        let _ = video.try_send(());
                        let _ = scene.try_send(());
                    }
                },
                Some(false),
            );
            drop((video, scene));
            audio
        });
        eprintln!("R07/M03/M04 active={active} completed_30s_exports_during_render={} completed_midi_scenes_during_render={}", observed[0], observed[1]);
        assert!(if active {
            observed.iter().all(|n| *n > 0)
        } else {
            observed == [0, 0]
        });
        phases.push(stats);
    }
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(&mp4)
        .output()
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    assert_eq!(String::from_utf8(probe.stdout).unwrap().trim(), "900");
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, missed deadlines, invalid blocks, allocations"
    );
}
