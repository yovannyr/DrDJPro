use super::*;
use crate::playback::{DeckId, DualDeckEngine};
use crate::test_support::{wav_file, wav_samples};
use std::f64::consts::TAU;

fn timing(ms: u64) -> PairTiming {
    PairTiming {
        starts_ms: [0, 0],
        length_ms: ms,
        offset_b_ms: 0,
        tempos: [1., 1.],
        pitches: [0, 0],
    }
}
fn tone(frame: usize, rate: u32, hz: f64) -> i16 {
    ((TAU * frame as f64 * hz / f64::from(rate)).sin() * 8_000.).round() as i16
}
fn level(pcm: &[f32], from_ms: usize, to_ms: usize, side: usize, hz: f64) -> f64 {
    let mut cosine = 0.;
    let mut sine = 0.;
    for (i, frame) in pcm[from_ms * 96..to_ms * 96]
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
    {
        let phase = TAU * i as f64 * hz / 48_000.;
        cosine += f64::from(frame[side]) * phase.cos();
        sine += f64::from(frame[side]) * phase.sin();
    }
    2. * cosine.hypot(sine) / ((to_ms - from_ms) * 48) as f64
}

#[test]
fn d01_pair_routes_independent_stereo_sources_only_to_headphones_and_respects_offset_and_eof() {
    let a = wav_samples(
        &(0..4800)
            .flat_map(|i| [tone(i, 24_000, 220.), 0])
            .collect::<Vec<_>>(),
        24_000,
        2,
    );
    let b = wav_samples(
        &(0..4800)
            .flat_map(|i| [0, tone(i, 48_000, 440.)])
            .collect::<Vec<_>>(),
        48_000,
        2,
    );
    let mut pair = timing(220);
    pair.offset_b_ms = 50;
    let clip = render(a.path(), b.path(), pair, &Cancellation::default()).unwrap();
    assert_eq!(
        (clip.rate, clip.channels, clip.pcm.len()),
        (RATE, 2, 220 * 96)
    );
    assert!(level(&clip.pcm, 60, 140, 0, 220.) > 0.09);
    assert!(level(&clip.pcm, 60, 140, 1, 440.) > 0.09);
    assert!(clip.pcm[..50 * 96]
        .as_chunks::<2>()
        .0
        .iter()
        .all(|f| f[1] == 0.));
    assert!(clip.pcm[150 * 96..]
        .as_chunks::<2>()
        .0
        .iter()
        .all(|f| f[1] == 0.));
    assert!(clip.pcm[200 * 96..].iter().all(|s| *s == 0.));
    let music = wav_file(4_000, 48_000);
    let make = || {
        let (mixer, output) = rodio::mixer::mixer(4, RATE);
        let engine = DualDeckEngine::from_mixer(mixer, 4, RATE);
        engine.load_a(music.path()).unwrap();
        engine.load_b(music.path()).unwrap();
        for id in [DeckId::A, DeckId::B] {
            engine.play(id);
        }
        (engine, output)
    };
    let (actual, mut output) = make();
    let (reference, mut reference_output) = make();
    actual.play_sample_preview(&clip).unwrap();
    let mut peak = [0.0_f32; 2];
    for frame in clip.pcm.as_chunks::<2>().0 {
        let f: [f32; 4] = std::array::from_fn(|_| output.next().unwrap());
        let r: [f32; 4] = std::array::from_fn(|_| reference_output.next().unwrap());
        assert_eq!(f[..2], r[..2]);
        assert_eq!(r[2..], [0., 0.]);
        for lane in 0..2 {
            assert!((f[2 + lane] - frame[lane] * 0.7).abs() < 1e-5);
            peak[lane] = peak[lane].max(f[2 + lane].abs());
        }
    }
    assert!(peak.iter().all(|x| *x > 0.06));
    for id in [DeckId::A, DeckId::B] {
        assert_eq!(
            actual.snapshot(id).position,
            reference.snapshot(id).position
        );
        assert!(actual.snapshot(id).playing);
    }
}

#[test]
fn d01_pair_tempo_changes_duration_while_pitch_sets_frequency_and_source_start() {
    let a = wav_samples(
        &(0..96_000)
            .map(|i| tone(i, 48_000, if i < 48_000 { 440. } else { 660. }))
            .collect::<Vec<_>>(),
        48_000,
        1,
    );
    let b = wav_file(0, 48_000);
    let mut pair = timing(1_400);
    pair.tempos[0] = 1.25;
    pair.pitches[0] = 12;
    let clip = render(a.path(), b.path(), pair, &Cancellation::default()).unwrap();
    assert!(level(&clip.pcm, 200, 600, 0, 880.) > 0.05);
    assert!(level(&clip.pcm, 200, 600, 0, 440.) < 0.01);
    assert!(level(&clip.pcm, 900, 1300, 0, 1320.) > 0.05);
    assert!(level(&clip.pcm, 900, 1300, 0, 880.) < 0.01);
    let mut early = timing(40);
    early.offset_b_ms = -10;
    early.starts_ms[1] = 7;
    let step: Vec<_> = (0..4800)
        .map(|i| if i < 17 * 48 + 2 { 0 } else { 10_000 })
        .collect();
    let step = wav_samples(&step, RATE, 1);
    let shifted = render(b.path(), step.path(), early, &Cancellation::default()).unwrap();
    assert_eq!(&shifted.pcm[..4], [0., 0., 0., 0.]);
    assert!(shifted.pcm[4] > 0.1);
    let cancel = Cancellation::default();
    cancel.request();
    assert!(render(a.path(), b.path(), pair, &cancel).is_err());
}

#[test]
fn d01_pair_downsampling_rejects_ultrasonic_alias_and_handles_short_shifted_excerpt() {
    let make = |hz| {
        wav_samples(
            &(0..19_200)
                .map(|i| tone(i, 192_000, hz))
                .collect::<Vec<_>>(),
            192_000,
            1,
        )
    };
    let audible = make(3_000.);
    let ultrasonic = make(60_000.);
    let silence = wav_file(0, 48_000);
    let cancel = Cancellation::default();
    let a = render(audible.path(), silence.path(), timing(100), &cancel).unwrap();
    let b = render(ultrasonic.path(), silence.path(), timing(100), &cancel).unwrap();
    assert!(level(&a.pcm, 10, 90, 0, 3_000.) > 0.09);
    assert!(b.pcm[10 * 96..90 * 96].iter().all(|s| s.abs() < 0.001));
    let mut short = timing(1);
    short.pitches = [-7, 0];
    let c = render(audible.path(), silence.path(), short, &cancel).unwrap();
    assert_eq!(c.pcm.len(), 96);
    assert!(c.pcm.iter().all(|s| s.is_finite()));
}
