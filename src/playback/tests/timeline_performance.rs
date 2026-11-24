use crate::{
    cancellation::Cancellation,
    set_timeline::{render_wav, Clip, Point, Project},
    test_support::wav_samples,
};
use std::{path::Path, sync::mpsc, time::Duration};

#[test]
#[ignore = "paired 30 s two-deck render with actual two-source WAV exports"]
fn d07_equal_duration_timeline_exports_with_two_live_decks() {
    let dir = tempfile::tempdir().unwrap();
    let left = wav_samples(&[4096; 16_000], 8000, 1);
    let right = wav_samples(&[-1024; 16_000], 8000, 1);
    let clip = |path: &Path, title: &str| Clip {
        path: path.to_owned(),
        title: title.to_owned(),
        start_ms: 0,
        source_ms: 0,
        length_ms: 2000,
        source_bpm: Some(120.),
        first_beat_ms: Some(0),
        gain: vec![
            Point { ms: 0, value: 0. },
            Point {
                ms: 1200,
                value: -3.,
            },
        ],
        eq: vec![
            Point { ms: 0, value: 3. },
            Point {
                ms: 2000,
                value: 0.,
            },
        ],
        fader: vec![],
    };
    let project = Project {
        version: 1,
        bpm: 120.,
        clips: vec![clip(left.path(), "left"), clip(right.path(), "right")],
    };
    let target = dir.path().join("offline.wav");
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        eprintln!("D07 offline two-source 2 s WAV export active={active}");
        let (audio, count) = std::thread::scope(|scope| {
            let (activate, start) = mpsc::sync_channel::<()>(1);
            let (ready, prepared) = mpsc::sync_channel(1);
            let (finish, stop) = mpsc::channel::<()>();
            let input = &project;
            let output = &target;
            let worker = scope.spawn(move || {
                if start.recv().is_err() {
                    return 0;
                }
                let cancel = Cancellation::default();
                let mut count = 0u64;
                loop {
                    render_wav(input, output, 0, 2000, &cancel).unwrap();
                    assert_eq!(output.metadata().unwrap().len(), 384044);
                    count += 1;
                    if count == 1 {
                        ready.send(()).unwrap();
                    }
                    if stop.recv_timeout(Duration::from_millis(750))
                        != Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        break;
                    }
                }
                count
            });
            let audio = super::render_bench::measure_routed(
                30,
                4,
                move |_| {
                    if active {
                        activate.send(()).unwrap();
                        prepared.recv_timeout(Duration::from_secs(20)).unwrap();
                    }
                },
                || {},
                Some(false),
            );
            drop(finish);
            (audio, worker.join().unwrap())
        });
        eprintln!(
            "D07 completed WAV exports={count} peak published export bytes={}",
            if count > 0 { 384044 } else { 0 }
        );
        assert!(if active { count >= 5 } else { count == 0 });
        phases.push(audio);
    }
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, deadline misses, signal errors, render allocations"
    );
}
