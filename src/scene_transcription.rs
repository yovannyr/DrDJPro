//! Worker-only monophonic bass/vocal transcription. Times are relative to the selected passage.
use crate::{cancellation::Cancellation, midi_file::Note, playback::clip_diagnosis::stream};
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::{path::Path, sync::Arc};
const MAX_MS: u64 = 60_000;

struct Mono {
    rate: u32,
    stride: usize,
    samples: Vec<f32>,
    sum: f32,
    count: usize,
}
impl Mono {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            stride: (rate / 8000).max(1) as usize,
            samples: Vec::new(),
            sum: 0.,
            count: 0,
        }
    }
    fn sample(&mut self, x: f32) {
        self.sum += x;
        self.count += 1;
        if self.count == self.stride {
            self.samples.push(self.sum / self.count as f32);
            self.count = 0;
            self.sum = 0.;
        }
    }
    fn finish(
        mut self,
        tempo: f64,
        ppq: u16,
        c: &Cancellation,
    ) -> Result<Vec<(Note, bool)>, String> {
        if self.count != 0 {
            self.samples.push(self.sum / self.count as f32);
        }
        let rate = f64::from(self.rate) / self.stride as f64;
        recognize(&self.samples, rate, tempo, ppq, c)
    }
}
fn check(tempo: f64, ppq: u16, c: &Cancellation) -> Result<(), String> {
    c.check()?;
    if !(4. ..=1000.).contains(&tempo) || !(1..=32767).contains(&ppq) {
        return Err("Invalid note transcription tempo or tick resolution".into());
    }
    Ok(())
}
pub(crate) fn transcribe(
    path: &Path,
    start_ms: u64,
    len_ms: u64,
    tempo: f64,
    ppq: u16,
    c: &Cancellation,
) -> Result<Vec<(Note, bool)>, String> {
    check(tempo, ppq, c)?;
    if !(1..=MAX_MS).contains(&len_ms) {
        return Err("Select up to 60 seconds of a monophonic passage".into());
    }
    let end = start_ms
        .checked_add(len_ms)
        .ok_or("Passage time overflow")?;
    let mut mono = None;
    let mut cursor = 0;
    let mut complete = false;
    stream(path, c, |channels, rate, pcm| {
        let frames = pcm.len() / usize::from(channels);
        let next = cursor + frames as u64;
        let frame = |ms| {
            u64::try_from((u128::from(ms) * u128::from(rate)).div_ceil(1000))
                .map_err(|_| "Source timestamp exceeds track range")
        };
        let first = frame(start_ms)?.max(cursor);
        let finish = frame(end)?;
        let last = finish.min(next);
        if first < last {
            let m = mono.get_or_insert_with(|| Mono::new(rate));
            let width = usize::from(channels);
            let block = &pcm[(first - cursor) as usize * width..(last - cursor) as usize * width];
            let side = if width == 2 {
                let e = block.as_chunks::<2>().0.iter().fold([0_f64; 2], |e, v| {
                    [
                        e[0] + f64::from(v[0]).powi(2),
                        e[1] + f64::from(v[1]).powi(2),
                    ]
                });
                usize::from(e[1] > e[0])
            } else {
                0
            };
            for f in block.chunks_exact(width) {
                m.sample(f[side]);
            }
        }
        cursor = next;
        complete = next >= finish;
        Ok(complete)
    })?;
    if !complete {
        return Err("Source recording ends before the selected scene interval.".into());
    }
    mono.ok_or("No audio at this passage position")?
        .finish(tempo, ppq, c)
}
pub(crate) fn transcribe_mono(
    samples: &[f32],
    rate: u32,
    tempo: f64,
    ppq: u16,
    c: &Cancellation,
) -> Result<Vec<(Note, bool)>, String> {
    check(tempo, ppq, c)?;
    if !(8000..=192000).contains(&rate) || samples.len() > rate as usize * 60 {
        return Err("Stem transcription supports up to 60 s at 8–192 kHz".into());
    }
    let mut mono = Mono::new(rate);
    for block in samples.chunks(4096) {
        c.check()?;
        for &v in block {
            if !v.is_finite() {
                return Err("Stem contains non-finite PCM".into());
            }
            mono.sample(v);
        }
    }
    mono.finish(tempo, ppq, c)
}

/// One FFT pair per frame produces the linear autocorrelation and the YIN normalized difference.
/// The zero padding prevents a short phrase from wrapping into the start of the analysis window.
struct Detector {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    bins: Vec<Complex32>,
    scratch: Vec<Complex32>,
    energy: Vec<f64>,
    difference: Vec<f64>,
    width: usize,
}
impl Detector {
    fn new(width: usize) -> Self {
        let size = (2 * width).next_power_of_two();
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let scratch = vec![
            Complex32::default();
            forward
                .get_inplace_scratch_len()
                .max(inverse.get_inplace_scratch_len())
        ];
        Self {
            forward,
            inverse,
            bins: vec![Complex32::default(); size],
            scratch,
            energy: vec![0.; width + 1],
            difference: vec![1.; width / 2 + 1],
            width,
        }
    }
    fn detect(&mut self, x: &[f32], rate: f64) -> Option<(u8, u8)> {
        self.bins.fill(Complex32::default());
        let mean = x.iter().map(|&v| f64::from(v)).sum::<f64>() / x.len() as f64;
        for (i, &v) in x.iter().enumerate() {
            let v = f64::from(v) - mean;
            self.bins[i].re = v as f32;
            self.energy[i + 1] = self.energy[i] + v * v;
        }
        let n = self.width;
        if self.energy[n] / (n as f64) < 1e-7 {
            return None;
        }
        self.forward
            .process_with_scratch(&mut self.bins, &mut self.scratch);
        for v in &mut self.bins {
            *v = Complex32::new(v.norm_sqr(), 0.);
        }
        self.inverse
            .process_with_scratch(&mut self.bins, &mut self.scratch);
        let lo = ((rate / 1600.).floor() as usize).max(2);
        let hi = ((rate / 30.).ceil() as usize).min(n / 2);
        if hi <= lo + 1 {
            return None;
        }
        let mut cumulative = 0.;
        for lag in 1..=hi {
            let corr = f64::from(self.bins[lag].re) / self.bins.len() as f64;
            let difference =
                (self.energy[n - lag] + self.energy[n] - self.energy[lag] - 2. * corr).max(0.);
            cumulative += difference;
            self.difference[lag] = if cumulative > 1e-10 {
                difference * lag as f64 / cumulative
            } else {
                1.
            };
        }
        let d = &self.difference;
        let local = |i: usize| d[i] <= d[i - 1] && d[i] < d[i + 1];
        let lag = (lo..hi).find(|&i| d[i] < 0.17 && local(i)).or_else(|| {
            (lo..hi)
                .filter(|&i| local(i) && d[i] < 0.36)
                .min_by(|&a, &b| d[a].total_cmp(&d[b]))
        })?;
        let curve = d[lag - 1] + d[lag + 1] - 2. * d[lag];
        let delta = if curve > 1e-9 {
            ((d[lag - 1] - d[lag + 1]) / (2. * curve)).clamp(-0.5, 0.5)
        } else {
            0.
        };
        let midi = 69. + 12. * ((rate / (lag as f64 + delta)) / 440.).log2();
        let rounded = midi.round();
        if !(0. ..=127.).contains(&rounded) {
            return None;
        }
        let detune = (midi - rounded).abs();
        let confidence = (100. * (1. - d[lag]) - 45. * detune).clamp(0., 100.);
        Some((rounded as u8, confidence.round() as u8))
    }
}
#[derive(Clone, Copy)]
struct Observation {
    midi: Option<u8>,
    rms: f32,
    confidence: u8,
}
fn recognize(
    pcm: &[f32],
    rate: f64,
    tempo: f64,
    ppq: u16,
    c: &Cancellation,
) -> Result<Vec<(Note, bool)>, String> {
    if pcm.len() < 64 {
        return Ok(Vec::new());
    }
    let width = ((rate * 0.09).round() as usize).min(pcm.len());
    let hop = (rate * 0.0125).round().max(1.) as usize;
    let mut detector = Detector::new(width);
    let mut obs = Vec::with_capacity(pcm.len().div_ceil(hop));
    for (index, block) in pcm.chunks(hop).enumerate() {
        c.check()?;
        let power = block.iter().map(|v| v * v).sum::<f32>() / block.len() as f32;
        let rms = power.sqrt();
        let center = index * hop + block.len() / 2;
        let from = center.saturating_sub(width / 2).min(pcm.len() - width);
        let found = if rms > 0.001 {
            detector.detect(&pcm[from..from + width], rate)
        } else {
            None
        };
        obs.push(Observation {
            midi: found.map(|p| p.0),
            confidence: found.map_or(0, |p| p.1),
            rms,
        });
    }
    // Bridge single ambiguous voiced hops between two matching notes; explicit silence remains a rest.
    for i in 1..obs.len().saturating_sub(1) {
        if obs[i].rms > 0.001
            && obs[i].confidence < 75
            && obs[i - 1].midi.is_some()
            && obs[i - 1].midi == obs[i + 1].midi
        {
            obs[i].midi = obs[i - 1].midi;
        }
    }
    let tick = |i: usize| {
        ((i * hop).min(pcm.len()) as f64 / rate * tempo * f64::from(ppq) / 60.).round() as u32
    };
    let mut notes = Vec::new();
    let mut first = 0;
    while first < obs.len() {
        let Some(pitch) = obs[first].midi else {
            first += 1;
            continue;
        };
        let mut end = first + 1;
        let mut strongest = obs[first].rms;
        while end < obs.len() && obs[end].midi == Some(pitch) {
            let previous = obs[end - 1].rms;
            if end >= first + 3
                && previous < strongest * 0.22
                && obs[end].rms > previous * 3.
                && obs[end].rms > 0.01
            {
                break;
            }
            strongest = strongest.max(obs[end].rms);
            end += 1;
        }
        let span = &obs[first..end];
        let start = tick(first);
        let duration = tick(end).saturating_sub(start);
        if span.len() >= 2 && duration > 0 {
            let confidence =
                span.iter().map(|o| u32::from(o.confidence)).sum::<u32>() / span.len() as u32;
            let velocity = midi_velocity(strongest);
            notes.push((
                Note {
                    start,
                    duration,
                    pitch,
                    velocity,
                    channel: 0,
                },
                confidence < 83 || span.len() < 4,
            ));
        }
        first = end;
    }
    c.check()?;
    Ok(notes)
}
fn midi_velocity(rms: f32) -> u8 {
    if !rms.is_finite() || rms <= 0. {
        return 1;
    }
    (127. * (rms / 0.5).min(1.).sqrt()).round().max(1.) as u8
}

#[cfg(test)]
mod acceptance {
    use super::*;

    #[test]
    fn m03_complete_silent_passage_is_distinct_from_a_short_recording() {
        let audio = crate::test_support::wav_samples(&[0; 1600], 8000, 1);
        let cancel = Cancellation::default();
        let read =
            |len| crate::playback::chord_preview::source_stereo(audio.path(), 100, len, &cancel);
        assert!(read(9600).is_err());
        assert!(transcribe(audio.path(), 100, 200, 120., 480, &cancel).is_err());
        let pcm = read(4800).unwrap();
        assert_eq!(pcm.len(), 9600);
        assert!(pcm.iter().all(|&v| v == 0.));
        assert!(transcribe(audio.path(), 100, 100, 120., 480, &cancel)
            .unwrap()
            .is_empty());
    }
}
