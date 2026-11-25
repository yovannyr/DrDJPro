use super::*;
use crate::cancellation::Cancellation;
use drdjpro_ui_spike::analysis::keys::{CamelotKey, KeyMode};

const RATE: u32 = 48_000;

fn frame4(output: &mut impl Iterator<Item = f32>) -> [f32; 4] {
    std::array::from_fn(|_| output.next().unwrap())
}

fn amplitude(samples: &[f32], hz: f64, rate: u32) -> f64 {
    let mut cosine = 0.;
    let mut sine = 0.;
    let mut weights = 0.;
    for (index, &sample) in samples.iter().enumerate() {
        let angle = std::f64::consts::TAU * index as f64;
        let weight = 0.5 - 0.5 * (angle / (samples.len() - 1) as f64).cos();
        let (s, c) = (angle * hz / f64::from(rate)).sin_cos();
        let weighted = f64::from(sample) * weight;
        sine += weighted * s;
        cosine += weighted * c;
        weights += weight;
    }
    2. * sine.hypot(cosine) / weights
}

#[test]
fn a07_reference_piano_has_the_selected_notes_and_loops_only_in_headphones() {
    // Opposite-phase stereo carries the passage, while the reference is centered. This separates
    // the two signals at the physical output without accessing synthesizer internals.
    let passage: Vec<i16> = (0..6_000)
        .flat_map(|i| {
            let phase = std::f64::consts::TAU * i as f64 / 24_000.;
            let sample = ([220., 261.6256, 329.6276]
                .into_iter()
                .map(|hz| (phase * hz).sin())
                .sum::<f64>()
                * 1600.) as i16;
            [sample, -sample]
        })
        .collect();
    let file = crate::test_support::wav_samples(&passage, 24_000, 2);
    let clip = PreviewClip::open(file.path(), Duration::ZERO, &Cancellation::default()).unwrap();
    let deck = wav_file(4096, RATE * 3);
    let make = || {
        let (mixer, output) = rodio::mixer::mixer(4, RATE);
        let engine = DualDeckEngine::from_mixer(mixer, 4, RATE);
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
    for _ in 0..480 {
        assert_eq!(frame4(&mut output), frame4(&mut baseline));
    }
    for (key, triad, expected, absent) in [
        ("8A", true, &[440., 523.251, 659.255][..], &[554.365][..]),
        ("8A", false, &[440.][..], &[523.251, 659.255][..]),
        ("8B", true, &[261.626, 329.628, 391.995][..], &[311.127][..]),
    ] {
        engine
            .play_key_reference(&clip, CamelotKey::parse(key).unwrap(), triad)
            .unwrap();
        let window = RATE as usize / 2;
        let mut reference = Vec::with_capacity(window);
        let mut passage = Vec::with_capacity(window);
        let heap = crate::test_alloc::measured_heap(|| {
            for _ in 0..window {
                let actual = frame4(&mut output);
                let normal = frame4(&mut baseline);
                assert_eq!(actual[..2], normal[..2]);
                assert!(normal[0] > 0.);
                reference.push((actual[2] + actual[3]) * 0.5);
                passage.push((actual[2] - actual[3]) * 0.5);
            }
        });
        assert_eq!(
            (heap.allocations, heap.reallocations, heap.deallocations),
            (0, 0, 0)
        );
        for &hz in expected {
            assert!(amplitude(&reference, hz, RATE) > 0.04, "{key} missing {hz}");
        }
        for &hz in absent {
            assert!(
                amplitude(&reference, hz, RATE) < 0.005,
                "{key} unexpected {hz}"
            );
        }
        // The file lasted only a quarter-second. Its second iteration must still be audible.
        let second_pass = &passage[window / 2..];
        for hz in [220., 261.626, 329.628] {
            assert!(
                amplitude(second_pass, hz, RATE) > 0.02,
                "missing looped passage note {hz}"
            );
        }
        engine.stop_tempo_preview();
        assert_eq!(frame4(&mut output), frame4(&mut baseline));
    }
}

#[test]
fn a07_key_preview_requires_an_independent_route_and_valid_tuning() {
    let file = wav_file(0, RATE / 4);
    let clip = PreviewClip::open(file.path(), Duration::ZERO, &Cancellation::default()).unwrap();
    let key = CamelotKey::parse("8A").unwrap();
    let (mixer, mut stereo) = rodio::mixer::mixer(2, RATE);
    let engine = DualDeckEngine::from_mixer(mixer, 2, RATE);
    assert!(engine.play_key_reference(&clip, key, true).is_err());
    assert!(stereo.by_ref().take(256).all(|s| s == 0.));
    let (mixer, mut output) = rodio::mixer::mixer(4, RATE);
    let engine = DualDeckEngine::from_mixer(mixer, 4, RATE);
    let invalid = CamelotKey {
        number: 0,
        mode: KeyMode::Minor,
    };
    assert!(engine.play_key_reference(&clip, invalid, true).is_err());
    assert!(crate::playback::key_reference::ReferenceVoice::new(&clip, key, true, 1000).is_err());
    engine.play_key_reference(&clip, key, true).unwrap();
    assert!(output.by_ref().take(512).any(|sample| sample != 0.));
    engine.set_output_pair(1).unwrap();
    assert!(engine.play_key_reference(&clip, key, true).is_err());
    output.by_ref().take(4).for_each(drop);
    assert!(output.by_ref().take(256).all(|sample| sample == 0.));
}

#[test]
#[ignore = "two 30 s measurements with both decks playing and four physical outputs"]
fn a07_equal_duration_reference_piano_with_two_decks() {
    let file = wav_file(4096, RATE / 3);
    let clip = PreviewClip::open(file.path(), Duration::ZERO, &Cancellation::default()).unwrap();
    let key = CamelotKey::parse("8A").unwrap();
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        eprintln!("A07 looping passage and A minor reference triad active={active}");
        let result = super::render_bench::measure_routed(
            30,
            4,
            |engine| {
                if active {
                    engine.play_key_reference(&clip, key, true).unwrap();
                }
            },
            || {},
            Some(active),
        );
        phases.push(result);
    }
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, missed deadlines, bad signal blocks, allocations"
    );
}
