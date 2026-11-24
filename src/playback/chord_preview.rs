//! Offline preparation of two aligned source excerpts for the separate headphone route.
use super::{clip_diagnosis, preview_clip, PreviewClip};
use crate::{cancellation::Cancellation, chord_timing::PairTiming};
use std::{path::Path, time::Duration};

const RATE: u32 = 48_000;
const PHASES: usize = 128;

/// Blackman-windowed sinc phases. Widen the low-pass kernel when reducing the source rate.
fn kernel(step: f64, radius: i64) -> Vec<Vec<f32>> {
    use std::f64::consts::PI;
    let cutoff = 1. / step.max(1.);
    (0..PHASES)
        .map(|phase| {
            let fraction = phase as f64 / PHASES as f64;
            let mut taps: Vec<_> = (-radius + 1..=radius)
                .map(|tap| {
                    let x = tap as f64 - fraction;
                    let sinc = if x.abs() < 1e-12 {
                        cutoff
                    } else {
                        (PI * cutoff * x).sin() / (PI * x)
                    };
                    let angle = PI * x / radius as f64;
                    (sinc * (0.42 + 0.5 * angle.cos() + 0.08 * (2. * angle).cos())) as f32
                })
                .collect();
            let sum: f32 = taps.iter().sum();
            for tap in &mut taps {
                *tap /= sum;
            }
            taps
        })
        .collect()
}

/// Holds at most one decoder block and a filter's worth of neighbouring stereo frames.
/// The requested output frame indexes the absolute source clock, including fractional offsets.
struct Resampler {
    first: f64,
    step: f64,
    target: usize,
    radius: i64,
    phases: Vec<Vec<f32>>,
    buffer: Vec<[f32; 2]>,
    base: i64,
    out: Vec<f32>,
}
impl Resampler {
    fn new(rate: u32, start_ms: f64, pitch: f64, target: usize) -> Self {
        let first = start_ms * f64::from(rate) / 1000.;
        let step = f64::from(rate) * pitch / f64::from(RATE);
        let direct = (step - 1.).abs() < 1e-12 && (first - first.round()).abs() < 1e-7;
        let radius = if direct {
            0
        } else {
            (12. * step.max(1.)).ceil() as i64
        };
        Self {
            first: if direct { first.round() } else { first },
            step,
            target,
            radius,
            phases: if direct {
                Vec::new()
            } else {
                kernel(step, radius)
            },
            buffer: Vec::new(),
            base: 0,
            out: Vec::new(),
        }
    }
    fn position(&self) -> f64 {
        self.first + (self.out.len() / 2) as f64 * self.step
    }
    fn push(&mut self, cursor: i64, channels: u16, pcm: &[f32]) {
        let width = usize::from(channels);
        let next = cursor + (pcm.len() / width) as i64;
        let wanted = (self.position().floor() as i64 - self.radius).max(0);
        if next <= wanted {
            return;
        }
        let discard = (wanted - self.base).clamp(0, self.buffer.len() as i64) as usize;
        self.buffer.drain(..discard);
        self.base += discard as i64;
        let from = cursor.max(wanted);
        if self.buffer.is_empty() {
            self.base = from;
        }
        self.buffer.extend(
            pcm[(from - cursor) as usize * width..]
                .chunks_exact(width)
                .map(|lane| [lane[0], lane[width - 1]]),
        );
        self.emit(next, false);
    }
    fn emit(&mut self, end: i64, eof: bool) {
        while self.out.len() / 2 < self.target {
            let pos = self.position();
            let center = pos.floor() as i64;
            if center >= end || (!eof && center + self.radius >= end) {
                break;
            }
            let frame = if self.radius == 0 {
                self.buffer[(center - self.base) as usize]
            } else {
                let frac = pos - center as f64;
                let phase = ((frac * PHASES as f64) as usize).min(PHASES - 1);
                let taps = &self.phases[phase];
                let left = center - self.radius + 1 - self.base;
                let mut acc = [0.; 2];
                if let Some(samples) = usize::try_from(left)
                    .ok()
                    .and_then(|i| self.buffer.get(i..i + taps.len()))
                {
                    for (sample, weight) in samples.iter().zip(taps) {
                        acc[0] += sample[0] * weight;
                        acc[1] += sample[1] * weight;
                    }
                } else {
                    for (offset, weight) in taps.iter().enumerate() {
                        if let Some(sample) = usize::try_from(left + offset as i64)
                            .ok()
                            .and_then(|i| self.buffer.get(i))
                        {
                            acc[0] += sample[0] * weight;
                            acc[1] += sample[1] * weight;
                        }
                    }
                }
                acc
            };
            self.out.extend(frame);
        }
    }
}

fn read_window(
    path: &Path,
    start_ms: f64,
    target: usize,
    pitch: f64,
    cancel: &Cancellation,
) -> Result<Vec<f32>, String> {
    let mut resampler = None;
    let mut cursor = 0;
    clip_diagnosis::stream(path, cancel, |channels, rate, pcm| {
        let r = resampler.get_or_insert_with(|| Resampler::new(rate, start_ms, pitch, target));
        r.push(cursor, channels, pcm);
        cursor += (pcm.len() / usize::from(channels)) as i64;
        Ok(r.out.len() / 2 == target)
    })?;
    cancel.check()?;
    if let Some(mut r) = resampler {
        r.emit(cursor, true);
        return Ok(r.out);
    }
    Ok(Vec::new())
}
/// Reuse the bounded band-limited decoder for original scene audio at 48 kHz.
pub(crate) fn source_stereo(
    path: &Path,
    start_ms: u64,
    frames: usize,
    cancel: &Cancellation,
) -> Result<Vec<f32>, String> {
    if frames == 0 || frames > 48_000 * 60 {
        return Err("Scene source window must be within one minute.".into());
    }
    let pcm = read_window(path, start_ms as f64, frames, 1., cancel)?;
    if pcm.len() != frames * 2 {
        return Err("Source recording ends before the selected scene interval.".into());
    }
    Ok(pcm)
}

fn change_duration(
    mut pcm: Vec<f32>,
    ratio: f64,
    cancel: &Cancellation,
) -> Result<Vec<f32>, String> {
    cancel.check()?;
    if pcm.is_empty() || (ratio - 1.).abs() < 1e-12 {
        return Ok(pcm);
    }
    let frames = pcm.len() / 2;
    let target = ((frames as f64 / ratio).round() as usize) * 2;
    // Signalsmith exact requires at least twice its synthesis latency (120-ms preset at 48 kHz).
    // Pad only short inputs; remove the excess immediately after processing.
    let minimum = 8192usize.max((8192. * ratio).ceil() as usize);
    if frames < minimum {
        pcm.resize(minimum * 2, 0.);
    }
    let mut processed = drdj_keylock::stretch(&pcm, 2, RATE, ratio).map_err(str::to_owned)?;
    cancel.check()?;
    processed.truncate(target);
    Ok(processed)
}

pub(crate) fn render(
    a: &Path,
    b: &Path,
    timing: PairTiming,
    cancel: &Cancellation,
) -> Result<PreviewClip, String> {
    timing.validate()?;
    cancel.check()?;
    let frames = timing.length_ms as usize * 48;
    let mut target = vec![0.; frames * 2];
    for (side, path) in [a, b].into_iter().enumerate() {
        let offset = if side == 1 { timing.offset_b_ms } else { 0 };
        let first = offset.max(0) as usize * 48;
        if first >= frames {
            continue;
        }
        let tempo = timing.tempos[side];
        let source_start =
            timing.starts_ms[side] as f64 + offset.min(0).unsigned_abs() as f64 * tempo;
        let pitch = 2_f64.powf(f64::from(timing.pitches[side]) / 12.);
        let ratio = tempo / pitch;
        let requested = ((frames - first) as f64 * ratio).ceil() as usize;
        let pcm = read_window(path, source_start, requested, pitch, cancel)?;
        let processed = change_duration(pcm, ratio, cancel)?;
        for (destination, source) in target[first * 2..]
            .chunks_mut(8192)
            .zip(processed.chunks(8192))
        {
            cancel.check()?;
            for (dst, sample) in destination.iter_mut().zip(source) {
                *dst += 0.45 * sample;
            }
        }
    }
    // Limit overshooting float masters and filter peaks with one gain for both sources.
    let mut peak = 0.9_f32;
    for block in target.chunks(8192) {
        cancel.check()?;
        for &sample in block {
            if !sample.is_finite() {
                return Err("Non-finite sample in paired chord preview".into());
            }
            peak = peak.max(sample.abs());
        }
    }
    if peak > 0.9 {
        let gain = 0.9 / peak;
        for block in target.chunks_mut(8192) {
            cancel.check()?;
            for sample in block {
                *sample *= gain;
            }
        }
    }
    preview_clip::finish(target, 2, RATE, Duration::ZERO, cancel)
}

#[cfg(test)]
mod tests;
