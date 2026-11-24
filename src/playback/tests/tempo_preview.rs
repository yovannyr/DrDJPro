use super::*;
use crate::beatgrid::BeatGrid;
use crate::cancellation::Cancellation;

fn read_frame(output: &mut impl Iterator<Item = f32>) -> [f32; 4] {
    std::array::from_fn(|_| output.next().unwrap())
}

#[test]
fn both_interpretations_restart_on_the_same_frame_and_accent_the_correct_beat() {
    let silence = wav_file(0, 48_000 * 5);
    let start = Duration::from_millis(900);
    let clip = PreviewClip::open(silence.path(), start, &Cancellation::default()).unwrap();
    let (mixer, mut output) = rodio::mixer::mixer(4, 48_000);
    let engine = DualDeckEngine::from_mixer(mixer, 4, 48_000);
    for bpm in [85., 170., 85.] {
        let grid = BeatGrid::new(bpm, Duration::from_secs(1)).unwrap();
        engine.play_tempo_preview(&clip, grid).unwrap();
        let expected: Vec<_> = (0..=4)
            .map(|n| ((0.1 + f64::from(n) * 60. / bpm) * 48_000.).round() as usize)
            .collect();
        let mut events = Vec::new();
        let mut last_sound = None;
        for frame in 0..=expected[4] + 1 {
            let [left, right, cue_l, cue_r] = read_frame(&mut output);
            assert_eq!([left, right], [0.; 2]);
            assert_eq!(cue_l, cue_r);
            if cue_l.abs() > 0.0001 {
                if last_sound.is_none_or(|prev| frame - prev > 200) {
                    events.push((frame, cue_l));
                }
                last_sound = Some(frame);
            }
        }
        assert_eq!(events.len(), 5);
        for (index, (frame, peak)) in events.into_iter().enumerate() {
            assert!(
                frame.abs_diff(expected[index]) <= 1,
                "{bpm} BPM beat {index}: {frame}"
            );
            let target = if index % 4 == 0 { 0.25 } else { 0.12 };
            assert!((peak - target).abs() < 0.01, "beat {index}: {peak}");
        }
    }
}

#[test]
fn master_matches_reference_during_switches_and_preview_keeps_file_speed() {
    let deck = wav_file(4096, 48_000);
    let samples: Vec<i16> = (0..24_000)
        .flat_map(|i| {
            let sample = if i % 200 < 50 { 4096i16 } else { -4096 };
            [sample, -sample]
        })
        .collect();
    let file = crate::test_support::wav_samples(&samples, 24_000, 2);
    let start = Duration::from_millis(17);
    let clip = PreviewClip::open(file.path(), start, &Cancellation::default()).unwrap();
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
    let (engine, mut test) = make();
    let (reference, mut baseline) = make();
    for _ in 0..480 {
        assert_eq!(read_frame(&mut test), read_frame(&mut baseline));
    }
    for bpm in [85., 170., 85.] {
        engine
            .play_tempo_preview(
                &clip,
                BeatGrid::new(bpm, Duration::from_millis(200)).unwrap(),
            )
            .unwrap();
        let heap = crate::test_alloc::measured_heap(|| {
            for frame in 0..1500 {
                let actual = read_frame(&mut test);
                let wanted = read_frame(&mut baseline);
                assert_eq!(actual[..2], wanted[..2]);
                assert!(actual[0] > 0.);
                // Integer positions in the 24 kHz source land on even 48 kHz output frames.
                if frame % 2 == 0 {
                    let i = (408 + frame / 2) * 2;
                    assert!((actual[2] - f32::from(samples[i]) / 32768. * 0.7).abs() < 1e-5);
                    assert!((actual[3] - f32::from(samples[i + 1]) / 32768. * 0.7).abs() < 1e-5);
                }
            }
        });
        assert_eq!(
            (heap.allocations, heap.reallocations, heap.deallocations),
            (0, 0, 0)
        );
        engine.stop_tempo_preview();
        assert_eq!(read_frame(&mut test), read_frame(&mut baseline));
        for id in [DeckId::A, DeckId::B] {
            let a = engine.snapshot(id);
            let b = reference.snapshot(id);
            assert_eq!(
                (a.position, a.playing, a.effective_speed, a.sync_factor),
                (b.position, b.playing, b.effective_speed, b.sync_factor)
            );
        }
    }
    let grid = BeatGrid::new(85., Duration::ZERO).unwrap();
    engine.play_tempo_preview(&clip, grid).unwrap();
    assert_eq!(test.next(), baseline.next());
    engine.set_output_pair(1).unwrap();
    reference.set_output_pair(1).unwrap();
    // The already started physical frame finishes on the old route.
    test.by_ref().take(3).for_each(drop);
    baseline.by_ref().take(3).for_each(drop);
    assert!(!engine.preview_route_available());
    assert!(engine.play_tempo_preview(&clip, grid).is_err());
    assert_eq!(read_frame(&mut test), read_frame(&mut baseline));
    let (stereo, mut out) = rodio::mixer::mixer(2, 48_000);
    let stereo = DualDeckEngine::from_mixer(stereo, 2, 48_000);
    assert!(stereo.play_tempo_preview(&clip, grid).is_err());
    assert!(out.by_ref().take(32).all(|s| s == 0.));
}

#[test]
fn preview_decode_limits_storage_and_honours_cancel_and_eof() {
    let cancel = Cancellation::default();
    let encoded =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/format-opus.opus");
    let opus = PreviewClip::open(&encoded, Duration::ZERO, &cancel).unwrap();
    let shifted = PreviewClip::open(&encoded, Duration::from_millis(10), &cancel).unwrap();
    assert_eq!(
        shifted.pcm[..],
        opus.pcm[480 * usize::from(opus.channels)..]
    );
    let wav = wav_file(4096, 48_000 * 10);
    let clip = PreviewClip::open(wav.path(), Duration::ZERO, &cancel).unwrap();
    assert_eq!(clip.pcm.len(), 48_000 * 8);
    assert!(PreviewClip::open(wav.path(), Duration::from_secs(12), &cancel).is_err());
    cancel.request();
    assert!(PreviewClip::open(wav.path(), Duration::ZERO, &cancel).is_err());
}

#[test]
#[ignore = "two real-time 30 second, 4-channel, 48 kHz deadline measurements"]
fn d04_equal_duration_headphone_switching_load() {
    let file = wav_file(4096, 96_000);
    let clip = PreviewClip::open(file.path(), Duration::ZERO, &Cancellation::default()).unwrap();
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        eprintln!("D04 preview active={active}");
        let (begin, ready) =
            std::sync::mpsc::sync_channel::<Arc<crate::playback::tempo_preview::PreviewControl>>(1);
        let (finished, stop) = std::sync::mpsc::channel::<()>();
        let clip = &clip;
        let result = std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let Ok(control) = ready.recv() else { return 0 };
                let mut switches = 0_usize;
                while stop.recv_timeout(Duration::from_millis(250))
                    == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    let bpm = if switches.is_multiple_of(2) {
                        170.
                    } else {
                        85.
                    };
                    let grid = BeatGrid::new(bpm, Duration::ZERO).unwrap();
                    let voice =
                        crate::playback::tempo_preview::Voice::new(clip, grid, 48_000).unwrap();
                    control.start(voice);
                    switches += 1;
                }
                switches
            });
            let result = super::render_bench::measure_routed(
                30,
                4,
                move |engine| {
                    if active {
                        engine
                            .play_tempo_preview(clip, BeatGrid::new(85., Duration::ZERO).unwrap())
                            .unwrap();
                        begin.send(engine.tempo_preview.clone()).unwrap();
                    }
                },
                || {},
                Some(active),
            );
            drop(finished);
            let switches = worker.join().unwrap();
            assert!(if active {
                switches >= 90
            } else {
                switches == 0
            });
            eprintln!("D04 control-thread switches={switches}");
            result
        });
        phases.push(result);
    }
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: D04 render overruns, deadlines, invalid output, allocations"
    );
}
