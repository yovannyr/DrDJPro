use super::*;
use crate::{cancellation::Cancellation, test_support::wav_samples};
use std::time::{Duration, Instant};

fn install(shared: &Shared, slot: usize, pcm: Prepared) {
    let (token, _) = shared.begin_load(slot).unwrap();
    shared.install(slot, token, &pcm).unwrap();
}
fn tone(hz: f32, rate: u32, frames: usize) -> Vec<i16> {
    (0..frames)
        .map(|i| (16000.0 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()) as i16)
        .collect()
}

pub(crate) fn start_benchmark_pattern(shared: &Shared) {
    // Positive left-only tone: the right master remains a deck reference, and the
    // rendered modulation distinguishes an audible sampler from a silent/DC bus.
    let sample = Prepared {
        path: "bench.wav".into(),
        rate: 48_000,
        samples: (0..48_000)
            .flat_map(|i| {
                let phase = std::f32::consts::TAU * (i % 128) as f32 / 128.;
                [0.25 + 0.0625 * phase.sin(), 0.]
            })
            .collect(),
    };
    install(shared, 0, sample);
    shared.set_gain(0.125).unwrap();
    let tuning = tools::Tuning {
        root: 57,
        correction: 0,
        digest: [1; 32],
    };
    let key = drdjpro_ui_spike::analysis::keys::CamelotKey::parse("8A").unwrap();
    let layout = tonal::Layout::make(0, &tuning, 0, key).unwrap();
    shared.use_layout(Some(&layout)).unwrap();
    let base = phrase::Pattern {
        bpm: 120,
        hits: (0..32)
            .map(|i| phrase::Hit {
                pad: (i % 4) as u8,
                micros: i * 250_000,
            })
            .collect(),
    };
    let fill = phrase::Pattern {
        bpm: 120,
        hits: (0..16)
            .map(|i| phrase::Hit {
                pad: (i % 4) as u8,
                micros: i * 125_000,
            })
            .collect(),
    };
    shared
        .pattern
        .start(&phrase::Patterns { base, fill }, shared)
        .unwrap();
    shared.pattern.arm().unwrap();
}

#[test]
fn m09_capture_uses_event_time_sorts_late_delivery_and_ignores_hits_after_end() {
    let at = Instant::now();
    let mut take = phrase::Take::new(false, 120, at).unwrap();
    take.end = Some(at + Duration::from_secs(1));
    for (pad, ms) in [(2, 375), (0, 0), (3, 750), (1, 250), (0, 1001)] {
        take.hit(pad, at + Duration::from_millis(ms));
    }
    assert_eq!(
        take.pattern.hits,
        [0, 250000, 375000, 750000].map(|us| phrase::Hit {
            pad: match us {
                0 => 0,
                250000 => 1,
                375000 => 2,
                _ => 3,
            },
            micros: us
        })
    );
}
#[test]
fn m08_fill_on_fourth_bar_then_base_and_next_cycle_cancel() {
    let bank = Shared::new(8_000).unwrap();
    for (slot, value) in [(0, 0.5), (1, -0.5)] {
        install(
            &bank,
            slot,
            Prepared {
                path: "hits.wav".into(),
                rate: 8_000,
                samples: vec![value; 160],
            },
        );
    }
    let base = phrase::Pattern {
        bpm: 240,
        hits: (0..4)
            .map(|bar| phrase::Hit {
                pad: 0,
                micros: bar * 1_000_000,
            })
            .collect(),
    };
    let fill = phrase::Pattern {
        bpm: 240,
        hits: vec![phrase::Hit { pad: 1, micros: 0 }],
    };
    bank.pattern
        .start(&phrase::Patterns { base, fill }, &bank)
        .unwrap();
    bank.pattern.arm().unwrap();
    let mut clock = phrase::Clock::default();
    for bar in 0..8 {
        for frame in 0..8000 {
            let v = clock.frame(&bank)[0];
            if frame == 30 {
                if bar == 3 {
                    assert!(v < 0.);
                    assert!(bank.pattern.filling());
                    bank.pattern.arm().unwrap();
                    bank.pattern.cancel_fill();
                } else {
                    assert!(v > 0.);
                    assert!(!bank.pattern.filling());
                }
            }
        }
    }
    bank.pattern.stop();
    assert_eq!(clock.frame(&bank), [0.; 2]);
}
#[test]
fn a04_tune_live_voice_preserves_phase_and_original_file() {
    let c = Cancellation::default();
    let actual = 440.0 * 2.0_f32.powf(25. / 1200.);
    let path = wav_samples(&tone(actual, 48_000, 48_000), 48_000, 1);
    let before = std::fs::read(path.path()).unwrap();
    let (sample, observation) = tools::read_sample(path.path(), &c).unwrap();
    let p = observation.pitch.unwrap();
    assert_eq!(p.midi, 69);
    assert!((p.cents - 25).abs() <= 5);
    let bank = Shared::new(48_000).unwrap();
    install(&bank, 0, sample);
    bank.play(0).unwrap();
    let mut voice = Voice::default();
    for _ in 0..2000 {
        voice.render(&bank, 0, 240, true, None);
    }
    let old = voice.phase;
    bank.set_pitch(0, -25).unwrap();
    let values: Vec<f32> = (0..12_000)
        .map(|_| voice.render(&bank, 0, 240, true, None)[0])
        .collect();
    assert_eq!(voice.phase, old + 12_000 * voice.increment);
    let down: Vec<f32> = values
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| v.iter().sum::<f32>() / 4.)
        .collect();
    let output = crate::source_monitor::analysis::pitch(&down, 12_000.).unwrap();
    assert_eq!(output.midi, 69);
    assert!(output.cents.abs() <= 5, "{output:?}");
    assert_eq!(std::fs::read(path.path()).unwrap(), before);
}
#[test]
fn m02_layers_preserve_stereo_level_and_twenty_ms_offset() {
    let c = Cancellation::default();
    let mut a = vec![0; 2400];
    a[0] = 8192;
    let mut b = vec![0; 2400];
    b[0] = 16384;
    let first = wav_samples(&a, 48000, 1);
    let second = wav_samples(&b, 48000, 1);
    let pair = tools::LayerPair {
        name: "Kick and clap".into(),
        second_path: second.path().to_owned(),
        mixes: [
            Default::default(),
            tools::LayerMix::new(20, 50, -100).unwrap(),
        ],
    };
    let pair: tools::LayerPair =
        serde_json::from_str(&serde_json::to_string(&pair).unwrap()).unwrap();
    let mixed = pair
        .render(Prepared::open(first.path(), &c).unwrap(), 48000, &c)
        .unwrap();
    assert_eq!(&mixed.samples[..2], &[0.25, 0.25]);
    assert_eq!(&mixed.samples[1920..1922], &[0.25, 0.]);
    assert_eq!(mixed.samples.iter().filter(|&&s| s != 0.).count(), 3);
    drop(second);
    assert!(pair
        .render(Prepared::open(first.path(), &c).unwrap(), 48000, &c)
        .is_err());
}
#[test]
fn m07_preview_scale_waits_for_activation_and_restores_mapping() {
    use drdjpro_ui_spike::analysis::keys::CamelotKey;
    let bank = Shared::new(48_000).unwrap();
    install(
        &bank,
        0,
        Prepared {
            path: "tone.wav".into(),
            rate: 48_000,
            samples: vec![0.5; 8000],
        },
    );
    let tuning = tools::Tuning {
        root: 57,
        correction: 0,
        digest: [0; 32],
    };
    let am = tonal::Layout::make(0, &tuning, 0, CamelotKey::parse("8A").unwrap()).unwrap();
    let c = tonal::Layout::make(0, &tuning, 1, CamelotKey::parse("8B").unwrap()).unwrap();
    assert_eq!(am.notes, [57, 59, 60, 62]);
    assert_eq!(c.notes, [60, 62, 64, 65]);
    assert!(!bank.ready(3));
    bank.use_layout(Some(&am)).unwrap();
    assert!(bank.ready(3));
    assert!(bank.pad_source(3).ratio > 1.);
    bank.use_layout(None).unwrap();
    assert!(!bank.ready(3));
}

#[test]
fn upward_pitch_filters_18khz_alias_on_live_and_headphone_routes() {
    use drdjpro_ui_spike::analysis::keys::CamelotKey;
    let bank = Shared::new(48_000).unwrap();
    let cancel = Cancellation::default();
    let tuning = tools::Tuning {
        root: 57,
        correction: 1200,
        digest: [0; 32],
    };
    let layout = tonal::Layout::make(0, &tuning, 0, CamelotKey::parse("8A").unwrap()).unwrap();
    for (hz, pass) in [(2000.0, true), (18000.0, false)] {
        let file = wav_samples(&tone(hz, 48_000, 16_000), 48_000, 1);
        install(&bank, 0, Prepared::open(file.path(), &cancel).unwrap());
        bank.set_pitch(0, 1200).unwrap();
        bank.play(0).unwrap();
        let mut voice = Voice::default();
        let live: Vec<_> = (0..3000)
            .map(|_| voice.render(&bank, 0, 240, false, None)[0])
            .collect();
        let pfl = bank
            .scale_clip(&layout, bank.source_token(0).unwrap(), &cancel)
            .unwrap();
        let peak = |data: &[f32]| data.iter().fold(0.0_f32, |max, &v| max.max(v.abs()));
        let a = peak(&live[500..]);
        let b = peak(&pfl.test_pcm()[1000..6000]);
        eprintln!("x2 source={hz:.0} Hz live_peak={a:.6} PFL_peak={b:.6}");
        if pass {
            assert!(a > 0.4 && b > 0.19);
        } else {
            assert!(a < 0.002 && b < 0.002);
        }
    }
}
