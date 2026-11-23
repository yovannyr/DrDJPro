use super::{frame_at, ClipReport};
use crate::cancellation::Cancellation;
use ebur128::{EbuR128, Mode};

pub(super) fn tap_count(rate: u32) -> usize {
    (rate.div_ceil(6000) as usize).clamp(2, 32)
}

/// Blend a short causal moving average into marked material only. The history
/// tracks unedited PCM so exporting in different packet sizes gives identical samples.
pub(super) struct Smoother<'a> {
    pub(super) frame: usize,
    report: &'a ClipReport,
    marker: usize,
    range: std::ops::Range<usize>,
    ramp: usize,
    ring: [[f32; 2]; 32],
    sum: [f64; 2],
    pos: usize,
    taps: usize,
}
impl<'a> Smoother<'a> {
    pub(super) fn new(report: &'a ClipReport, frame: usize) -> Self {
        let marker = report
            .markers
            .partition_point(|m| frame_at(m.end, report.rate) <= frame);
        let mut smoother = Self {
            frame,
            report,
            marker,
            range: 0..0,
            ramp: (report.rate as usize / 500).max(1),
            ring: [[0.; 2]; 32],
            sum: [0.; 2],
            pos: 0,
            taps: tap_count(report.rate),
        };
        smoother.advance();
        smoother
    }
    fn advance(&mut self) {
        self.range = self
            .report
            .markers
            .get(self.marker)
            .map_or(usize::MAX..usize::MAX, |m| {
                frame_at(m.start, self.report.rate)..frame_at(m.end, self.report.rate)
            });
    }
    pub(super) fn apply(&mut self, pcm: &mut [f32]) {
        for samples in pcm.chunks_exact_mut(usize::from(self.report.channels)) {
            if self.frame >= self.range.end {
                self.marker += 1;
                self.advance();
            }
            let wet = if self.range.contains(&self.frame) {
                let edge = (self.frame - self.range.start + 1).min(self.range.end - self.frame);
                0.8 * edge.min(self.ramp) as f32 / self.ramp as f32
            } else {
                0.
            };
            for (ch, sample) in samples.iter_mut().enumerate() {
                self.sum[ch] += f64::from(*sample) - f64::from(self.ring[self.pos][ch]);
                self.ring[self.pos][ch] = *sample;
                if wet > 0. {
                    let smooth = (self.sum[ch] / self.taps as f64) as f32;
                    *sample = (*sample) * (1. - wet) + smooth * wet;
                }
            }
            self.pos = (self.pos + 1) % self.taps;
            self.frame += 1;
        }
    }
}

/// Integrated BS.1770 loudness and sample peak of the complete audition window.
fn measure(
    pcm: &[f32],
    channels: u16,
    rate: u32,
    cancel: &Cancellation,
) -> Result<(f64, f64), String> {
    let mut meter = EbuR128::new(u32::from(channels), rate, Mode::I).map_err(|e| e.to_string())?;
    let mut peak = 0.0f32;
    for block in pcm.chunks(4096 * usize::from(channels)) {
        cancel.check()?;
        meter.add_frames_f32(block).map_err(|e| e.to_string())?;
        peak = block.iter().fold(peak, |p, x| p.max(x.abs()));
    }
    let loudness = meter.loudness_global().map_err(|e| e.to_string())?;
    if !loudness.is_finite() {
        return Err("Passage is too short or too quiet to match integrated loudness".into());
    }
    Ok((loudness, f64::from(peak)))
}
pub(super) fn match_loudness(
    raw: &mut [f32],
    edited: &mut [f32],
    channels: u16,
    rate: u32,
    cancel: &Cancellation,
) -> Result<(), String> {
    let (a, peak_a) = measure(raw, channels, rate, cancel)?;
    let (b, peak_b) = measure(edited, channels, rate, cancel)?;
    let target = a.min(b);
    let gains = [
        10.0f64.powf((target - a) / 20.),
        10.0f64.powf((target - b) / 20.),
    ];
    let headroom = 0.98 / (peak_a * gains[0]).max(peak_b * gains[1]).max(0.98);
    for (pcm, gain) in [(raw, gains[0]), (edited, gains[1])] {
        let gain = (gain * headroom) as f32;
        for block in pcm.chunks_mut(8192) {
            cancel.check()?;
            for x in block {
                *x *= gain;
            }
        }
    }
    Ok(())
}
