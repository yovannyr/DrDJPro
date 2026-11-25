use crate::{
    cancellation::Cancellation,
    chord_passage,
    chord_timing::PairTiming,
    playback::{chord_preview, tempo_preview},
    test_support::{wav_samples, TempFile},
};
use std::{
    path::Path,
    sync::{mpsc, Arc},
    time::Duration,
};

fn source(root: f32) -> TempFile {
    let samples = (0..72_000)
        .map(|n| {
            let time = n as f32 / 12_000.;
            let tonic = if n < 36_000 { root } else { root * 1.5 };
            let phase = std::f32::consts::TAU * time * tonic;
            (3000. * (phase.sin() + (phase * 1.1892).sin() + (phase * 1.4983).sin())) as i16
        })
        .collect::<Vec<_>>();
    wav_samples(&samples, 12_000, 1)
}

fn analyze_and_render(a: &Path, b: &Path) -> crate::playback::PreviewClip {
    let timing = PairTiming {
        starts_ms: [200, 100],
        length_ms: 3900,
        offset_b_ms: 240,
        tempos: [1., 1.10],
        pitches: [0, 1],
    };
    let cancel = Cancellation::default();
    for (side, path) in [a, b].into_iter().enumerate() {
        let (start, end) = timing.source_window(side).unwrap();
        let profile = chord_passage::analyze(path, start, end - start, &cancel).unwrap();
        assert!(!profile.events.is_empty());
    }
    chord_preview::render(a, b, timing, &cancel).unwrap()
}

#[test]
#[ignore = "paired 30 s four-channel render with two chord scans and a transposed PFL mix"]
fn d01_equal_duration_chord_analysis_and_audition_with_two_decks() {
    let a = source(220.);
    let b = source(293.665);
    let (a, b) = (a.path(), b.path());
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        eprintln!("D01 chord scans and matched pair PFL active={active}");
        let (audio, auditions) = std::thread::scope(|scope| {
            let (send, receive) = mpsc::sync_channel::<Arc<tempo_preview::PreviewControl>>(1);
            let (finish, stop) = mpsc::channel::<()>();
            let worker = scope.spawn(move || {
                let Ok(control) = receive.recv() else {
                    return 0;
                };
                let mut count = 0;
                while stop.recv_timeout(Duration::from_millis(500))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    let pair = analyze_and_render(a, b);
                    control.start_sample(tempo_preview::Excerpt::new(&pair, 48_000).unwrap());
                    count += 1;
                }
                count
            });
            let audio = super::render_bench::measure_routed(
                30,
                4,
                move |engine| {
                    if active {
                        let first = analyze_and_render(a, b);
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
        eprintln!("D01 complete scan pairs with PFL renders={auditions}");
        assert!(if active {
            auditions >= 5
        } else {
            auditions == 0
        });
        phases.push(audio);
    }
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, deadline misses, signal errors, render allocations"
    );
}
