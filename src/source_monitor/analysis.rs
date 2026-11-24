use crate::{audio_analyzer, cancellation::Cancellation, local_harmony::peak_chroma};
use rustfft::{num_complex::Complex32, FftPlanner};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Pitch {
    pub midi: i32,
    pub cents: i32,
    pub confidence: u8,
}
pub(crate) fn note_name(midi: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
    ];
    format!(
        "{}{}",
        NAMES[midi.rem_euclid(12) as usize],
        midi.div_euclid(12) - 1
    )
}

/// YIN cumulative normalized difference plus a harmonic-energy gate for a single sustained note.
pub(crate) fn pitch(samples: &[f32], rate: f64) -> Option<Pitch> {
    if !(6000. ..=32_000.).contains(&rate) {
        return None;
    }
    let n = samples.len().min(4096);
    if n < 1024 {
        return None;
    }
    let x = &samples[samples.len() - n..];
    if x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / (n as f64) < 1e-7 {
        return None;
    }
    let last = ((rate / 55.).ceil() as usize).min(n / 3);
    let first = ((rate / 1600.).floor() as usize).max(2);
    let mut norm = vec![1.; last + 1];
    let mut total = 0.;
    for lag in 1..=last {
        let difference: f64 = x[..n - last]
            .iter()
            .zip(&x[lag..])
            .map(|(&a, &b)| f64::from(a - b).powi(2))
            .sum();
        total += difference;
        if total > 1e-9 {
            norm[lag] = difference * lag as f64 / total;
        }
    }
    let center = (first..last)
        .find(|&i| norm[i] < 0.12 && norm[i] <= norm[i - 1] && norm[i] <= norm[i + 1])?;
    let (left, mid, right) = (norm[center - 1], norm[center], norm[center + 1]);
    let curvature = left + right - 2. * mid;
    let delta = if curvature > 1e-9 {
        ((left - right) / (2. * curvature)).clamp(-0.5, 0.5)
    } else {
        0.
    };
    let hz = rate / (center as f64 + delta);
    let size = n.next_power_of_two();
    let mut data = vec![Complex32::default(); size];
    for (i, (&v, s)) in x.iter().zip(&mut data).enumerate() {
        s.re = v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos());
    }
    FftPlanner::new().plan_fft_forward(size).process(&mut data);
    let bin_hz = rate / size as f64;
    let (mut harmonic, mut energy) = (0., 0.);
    for (i, bin) in data[1..size / 2].iter().enumerate() {
        let freq = (i + 1) as f64 * bin_hz;
        if freq < 40. {
            continue;
        }
        let e = f64::from(bin.norm_sqr());
        energy += e;
        let k = (freq / hz).round();
        if k >= 1. && (freq - k * hz).abs() < (2.2 * bin_hz).max(0.01 * freq) {
            harmonic += e;
        }
    }
    if energy <= 1e-8 || harmonic / energy < 0.8 {
        return None;
    }
    let exact = 69. + 12. * (hz / 440.).log2();
    let midi = exact.round() as i32;
    (0..=127).contains(&midi).then_some(Pitch {
        midi,
        cents: ((exact - f64::from(midi)) * 100.).round() as i32,
        confidence: (100. * (1. - mid) * (harmonic / energy))
            .clamp(0., 100.)
            .round() as u8,
    })
}

pub(super) fn describe(
    samples: &[f32],
    rate: f64,
    cancel: &Cancellation,
) -> Result<String, String> {
    cancel.check()?;
    let recent = samples.len().saturating_sub((rate * 0.4) as usize);
    let power = samples[recent..]
        .iter()
        .map(|&v| f64::from(v) * f64::from(v))
        .sum::<f64>()
        / (samples.len() - recent).max(1) as f64;
    if power < 1e-7 {
        return Ok("Silence · BPM unknown · key unknown · note unknown".into());
    }
    let note = pitch(samples, rate).map_or_else(
        || "note unknown".to_owned(),
        |p| {
            format!(
                "{} ({:+} ct, {}%)",
                note_name(p.midi),
                p.cents,
                p.confidence
            )
        },
    );
    if samples.len() < (rate * 8.1) as usize {
        return Ok(format!("Collecting 8.25 s of audio · {note}"));
    }
    let hop = (rate / 200.).round().max(1.) as usize;
    // Sample onsets every 5 ms, integrating 40 ms of energy at each step.
    // Shorter, disjoint RMS windows turn interference between notes of a sustained
    // chord into false percussive onsets, especially for slower passages.
    const HOPS: usize = 8;
    let mut history = [0.0_f64; HOPS];
    let mut power = 0.0_f64;
    let env: Vec<_> = samples
        .chunks_exact(hop)
        .enumerate()
        .map(|(i, chunk)| {
            let current = chunk.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / hop as f64;
            let slot = &mut history[i % HOPS];
            power += current - *slot;
            *slot = current;
            (power.max(0.) / (i + 1).min(HOPS) as f64).sqrt() as f32
        })
        .collect();
    let bpm = audio_analyzer::estimate_bpm_cancellable(&env, rate / hop as f64, cancel)
        .map_err(|e| e.to_string())?;
    let mut chroma = [0_f64; 12];
    if rate >= 6000. {
        const N: usize = 2048;
        let fft = FftPlanner::new().plan_fft_forward(N);
        let mut data = vec![Complex32::default(); N];
        let mut scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        let window: Vec<_> = (0..N)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (N - 1) as f32).cos())
            .collect();
        let mut power = vec![0.; N / 2];
        for chunk in samples.as_chunks::<N>().0 {
            cancel.check()?;
            for ((out, &v), &w) in data.iter_mut().zip(chunk).zip(&window) {
                *out = Complex32::new(v * w, 0.);
            }
            fft.process_with_scratch(&mut data, &mut scratch);
            for (out, v) in power.iter_mut().zip(&data) {
                *out = v.norm_sqr();
            }
            if let Some(c) = peak_chroma(&power, (rate / N as f64) as f32, 0.16) {
                for (sum, v) in chroma.iter_mut().zip(c) {
                    *sum += f64::from(v);
                }
            }
        }
    }
    let max = chroma.iter().copied().fold(0., f64::max);
    let key = if max > 0. && chroma.iter().filter(|&&x| x > max * 0.12).count() >= 3 {
        audio_analyzer::estimate_key(&chroma)
    } else {
        None
    };
    Ok(format!(
        "{} · {} · {note}",
        bpm.map_or_else(|| "BPM unknown".to_string(), |x| format!("{x:.1} BPM")),
        key.map_or_else(|| "key unknown".to_string(), |k| format!("key {k}"))
    ))
}

#[cfg(test)]
mod qualification {
    use super::*;
    #[test]
    fn a03_tonal_rhythm_known_sources_silence_and_single_note() {
        let rate = 12_000.0;
        let cancel = Cancellation::default();
        let tone = |f: f64, t: f64| (std::f64::consts::TAU * f * t).sin();
        for (bpm, tones, key) in [
            (120.0, [261.626, 329.628, 391.995], "8B"),
            (90.0, [369.994, 440.0, 554.365], "11A"),
        ] {
            let signal: Vec<f32> = (0..99_000)
                .map(|i| {
                    let t = i as f64 / rate;
                    let beat = (t * bpm / 60.).fract();
                    let env = 0.11 + 0.7 * (-8.0 * beat).exp();
                    (env * (tone(tones[0], t) + 0.8 * tone(tones[1], t) + 0.75 * tone(tones[2], t))
                        / 3.0) as f32
                })
                .collect();
            let result = describe(&signal, rate, &cancel).unwrap();
            let found: f64 = result
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap_or(0.);
            assert!(
                (found - bpm).abs() <= 2. && result.contains(&format!("key {key}")),
                "{result} for {bpm} {key}"
            );
        }
        let single: Vec<f32> = (0..99_000)
            .map(|i| tone(440., i as f64 / rate) as f32)
            .collect();
        let result = describe(&single, rate, &cancel).unwrap();
        assert!(
            result.contains("A4") && result.contains("key unknown"),
            "{result}"
        );
        assert!(describe(&[0.; 4096], rate, &cancel)
            .unwrap()
            .starts_with("Silence"));
    }
}
