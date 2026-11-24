//! Worker-side harmonic/percussive overview. The decoder and FFT never enter a deck callback.
use crate::{cancellation::Cancellation, playback::clip_diagnosis};
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::{collections::VecDeque, path::Path, sync::Arc};

const N: usize = 512;
const HOP: usize = 128;
const RADIUS: usize = 6;
const DISPLAY: usize = 256;

#[derive(Clone, Copy, Default)]
pub(crate) struct Levels {
    pub percussion: f32,
    pub tonal: f32,
}
impl Levels {
    fn merge(&mut self, other: Self) {
        self.percussion = self.percussion.max(other.percussion);
        self.tonal = self.tonal.max(other.tonal);
    }
}

struct Envelope {
    bins: Vec<Levels>,
    stride: usize,
    frames: usize,
}
impl Envelope {
    fn push(&mut self, value: Levels) {
        let mut index = self.frames / self.stride;
        if index == 4096 {
            for i in 0..2048 {
                let mut pair = self.bins[2 * i];
                pair.merge(self.bins[2 * i + 1]);
                self.bins[i] = pair;
            }
            self.bins.truncate(2048);
            self.stride *= 2;
            index = 2048;
        }
        if index == self.bins.len() {
            self.bins.push(value);
        } else {
            self.bins[index].merge(value);
        }
        self.frames += 1;
    }
    fn display(&self) -> Vec<Levels> {
        let count = self.frames.min(DISPLAY);
        let mut out = vec![Levels::default(); count];
        for (i, value) in self.bins.iter().enumerate() {
            let start = i * self.stride * count / self.frames;
            let end = ((i + 1) * self.stride)
                .min(self.frames)
                .saturating_mul(count)
                .div_ceil(self.frames);
            for bin in &mut out[start..end] {
                bin.merge(*value);
            }
        }
        let max = out
            .iter()
            .map(|v| v.percussion.max(v.tonal))
            .fold(1e-12_f32, f32::max);
        for v in &mut out {
            v.percussion /= max;
            v.tonal /= max;
        }
        out
    }
}

struct Analyzer {
    decimation: usize,
    sample_count: usize,
    sample_sum: Complex32,
    samples: VecDeque<Complex32>,
    fft: Arc<dyn Fft<f32>>,
    window: [f32; N],
    frame: Vec<Complex32>,
    scratch: Vec<Complex32>,
    history: VecDeque<Vec<f32>>,
    pending: usize,
    envelope: Envelope,
}
impl Analyzer {
    fn new(rate: u32) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(N);
        Self {
            decimation: (rate as usize / 16000).max(1),
            sample_count: 0,
            sample_sum: Complex32::default(),
            samples: std::iter::repeat_n(Complex32::default(), N / 2).collect(),
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            window: std::array::from_fn(|i| {
                0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()
            }),
            frame: vec![Complex32::default(); N],
            history: VecDeque::new(),
            pending: 0,
            envelope: Envelope {
                bins: Vec::new(),
                stride: 1,
                frames: 0,
            },
        }
    }
    fn sample(&mut self, sample: Complex32) {
        self.samples.push_back(sample);
        if self.samples.len() == N {
            for ((dst, src), win) in self
                .frame
                .iter_mut()
                .zip(&self.samples)
                .zip(self.window.iter())
            {
                *dst = *src * *win;
            }
            self.fft
                .process_with_scratch(&mut self.frame, &mut self.scratch);
            // Complex packing preserves opposite-phase stereo through mirrored bin powers.
            let magnitudes: Vec<f32> = (0..=N / 2)
                .map(|i| (self.frame[i].norm_sqr() + self.frame[(N - i) % N].norm_sqr()).sqrt())
                .collect();
            self.samples.drain(..HOP);
            self.history.push_back(magnitudes);
            if self.history.len() > self.pending + RADIUS {
                self.consume();
            }
        }
    }
    fn consume(&mut self) {
        let value = split(&self.history, self.pending);
        self.envelope.push(value);
        if self.pending == RADIUS {
            self.history.pop_front();
        } else {
            self.pending += 1;
        }
    }
    fn push(&mut self, channels: u16, pcm: &[f32]) {
        for frame in pcm.chunks_exact(usize::from(channels)) {
            self.sample_sum += Complex32::new(frame[0], frame[frame.len() - 1]);
            self.sample_count += 1;
            if self.sample_count == self.decimation {
                self.sample(self.sample_sum / self.sample_count as f32);
                self.sample_count = 0;
                self.sample_sum = Complex32::default();
            }
        }
    }
    fn finish(mut self) -> Vec<Levels> {
        if self.sample_count > 0 {
            self.sample(self.sample_sum / self.sample_count as f32);
        }
        for _ in 0..N / 2 {
            self.sample(Complex32::default());
        }
        while self.pending < self.history.len() {
            self.consume();
        }
        self.envelope.display()
    }
}

/// Median over time estimates persistence. Broadband neighbourhood energy and energy above
/// that persistent floor identify attacks even when a held tone occupies the same register.
fn split(history: &VecDeque<Vec<f32>>, center: usize) -> Levels {
    let current = &history[center];
    let mut tonal_power = 0.0;
    let mut drum_power = 0.0;
    let mut temporal = [0.0; 2 * RADIUS + 1];
    let mut spectral = [0.0; 9];
    for (i, &mag) in current.iter().enumerate() {
        if mag <= 1e-9 {
            continue;
        }
        for (out, frame) in temporal.iter_mut().zip(history) {
            *out = frame[i];
        }
        let count = history.len();
        let stable = *temporal[..count]
            .select_nth_unstable_by(count / 2, f32::total_cmp)
            .1;
        for (j, out) in spectral.iter_mut().enumerate() {
            *out = current[(i + j).saturating_sub(4).min(current.len() - 1)];
        }
        let broad = *spectral.select_nth_unstable_by(4, f32::total_cmp).1;
        let attack = (mag - stable).max(0.0);
        let percussive = attack.max(broad * 0.75);
        let drum_mask = percussive.powi(2) / (percussive.powi(2) + stable.powi(2) + 1e-16);
        drum_power += mag * mag * drum_mask;
        tonal_power += mag * mag * (1. - drum_mask);
    }
    Levels {
        percussion: drum_power.sqrt(),
        tonal: tonal_power.sqrt(),
    }
}

pub(crate) fn analyze(path: &Path, cancel: &Cancellation) -> Result<Vec<Levels>, String> {
    let mut data = None;
    clip_diagnosis::stream(path, cancel, |channels, rate, pcm| {
        data.get_or_insert_with(|| Analyzer::new(rate))
            .push(channels, pcm);
        Ok(false)
    })?;
    cancel.check()?;
    Ok(data.ok_or("Audio contains no frames.")?.finish())
}

#[cfg(test)]
mod qualification {
    use super::*;
    use crate::test_support::wav_samples;

    #[test]
    fn a05_transients_tone_and_silence_share_source_time() {
        let rate = 16_000;
        let mut samples = vec![0; 4 * rate];
        let phase = |i: usize| std::f32::consts::TAU * 220.0 * i as f32 / rate as f32;
        for (i, sample) in samples[2 * rate..].iter_mut().enumerate() {
            *sample = (6_000.0 * phase(i).sin()) as i16;
        }
        // Identical short 220 Hz attacks, first isolated and then over the sustained 220 Hz tone.
        let attacks = [1_600, 4_800, 9_600, 14_400];
        for start in attacks {
            for i in 0..rate * 15 / 1_000 {
                let kick = (24_000.0 * (-(i as f32) / 48.0).exp() * phase(i).cos()) as i16;
                samples[start + i] += kick;
                samples[3 * rate + start + i] += kick;
            }
        }
        let wav = wav_samples(&samples, rate as u32, 1);
        let mut levels = Vec::new();
        let heap = crate::test_alloc::measured_heap(|| {
            levels = analyze(wav.path(), &Cancellation::default()).unwrap();
        });
        assert_eq!(levels.len(), DISPLAY);
        for start in attacks {
            let near = |frame: usize| {
                let bin = frame * DISPLAY / samples.len();
                &levels[bin - 4..=bin + 2]
            };
            assert!(near(start)
                .iter()
                .any(|v| v.percussion > 0.05 && v.percussion > v.tonal));
            assert!(near(3 * rate + start)
                .iter()
                .any(|v| v.percussion > 0.05 && v.tonal > 0.1));
        }
        assert!(levels[2 * DISPLAY / 4 + 12..3 * DISPLAY / 4 - 12]
            .iter()
            .all(|v| v.tonal > 5.0 * v.percussion && v.tonal > 0.2));
        assert!(levels[DISPLAY / 4 + 15..DISPLAY / 2 - 15]
            .iter()
            .all(|v| v.tonal == 0. && v.percussion == 0.));
        eprintln!(
            "A05 measured worker heap peak={} bytes, overview={} bins",
            heap.peak_bytes,
            levels.len()
        );
    }
}
