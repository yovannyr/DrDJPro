use super::MAX_HALF_SECONDS;
use crate::cancellation::Cancellation;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::Arc;

pub(super) type Chroma = [f32; 12];

/// The two real channel signals share one complex FFT. Summing the mirrored bin powers
/// recovers both channels' spectral energy, including opposite-phase stereo.
pub(super) struct Spectrum {
    rate: u32,
    frames: u64,
    next_half: u64,
    bins: Vec<Option<Chroma>>,
    fft: Arc<dyn Fft<f32>>,
    data: Vec<Complex32>,
    scratch: Vec<Complex32>,
    hann: Vec<f32>,
    power: Vec<f32>,
    filled: usize,
    peak: f32,
    chroma: Chroma,
    tonal_frames: usize,
    sampled_frames: usize,
}
impl Spectrum {
    pub(super) fn new(rate: u32) -> Self {
        let n = (rate as usize / 7).next_power_of_two().clamp(2048, 32768);
        let fft = FftPlanner::new().plan_fft_forward(n);
        Self {
            rate,
            frames: 0,
            next_half: u64::from(rate).div_ceil(2),
            bins: Vec::new(),
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            data: vec![Complex32::default(); n],
            power: vec![0.; n / 2],
            hann: (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (n - 1) as f32).cos())
                .collect(),
            filled: 0,
            peak: 0.,
            chroma: [0.; 12],
            tonal_frames: 0,
            sampled_frames: 0,
        }
    }
    pub(super) fn push(
        &mut self,
        channels: u16,
        pcm: &[f32],
        cancel: &Cancellation,
    ) -> Result<(), String> {
        for frame in pcm.chunks_exact(usize::from(channels)) {
            let a = frame[0];
            let b = frame.get(1).copied().unwrap_or(0.);
            self.peak = self.peak.max(a.abs()).max(b.abs());
            self.data[self.filled] = Complex32::new(a, b);
            self.filled += 1;
            self.frames += 1;
            if self.filled == self.data.len() {
                cancel.check()?;
                self.transform();
            }
            if self.frames == self.next_half {
                self.close_half()?;
                self.next_half = ((self.bins.len() as u64 + 1) * u64::from(self.rate)).div_ceil(2);
            }
        }
        Ok(())
    }
    fn transform(&mut self) {
        let n = self.data.len();
        let count = self.filled;
        self.filled = 0;
        let peak = std::mem::take(&mut self.peak);
        if count < n / 3 {
            return;
        }
        self.sampled_frames += count;
        if peak < 0.0001 {
            return;
        }
        for (i, value) in self.data[..count].iter_mut().enumerate() {
            // A shortened final FFT has its own complete Hann taper before zero padding.
            *value = (*value / peak) * self.hann[i * (n - 1) / (count - 1)];
        }
        self.data[count..].fill(Complex32::default());
        self.fft
            .process_with_scratch(&mut self.data, &mut self.scratch);
        for (i, energy) in self.power.iter_mut().enumerate().skip(1) {
            *energy = self.data[i].norm_sqr() + self.data[n - i].norm_sqr();
        }
        if let Some(chroma) = peak_chroma(&self.power, self.rate as f32 / n as f32, 0.27) {
            for (sum, pitch) in self.chroma.iter_mut().zip(chroma) {
                *sum += pitch * count as f32;
            }
            self.tonal_frames += count;
        }
    }
    fn close_half(&mut self) -> Result<(), String> {
        if self.bins.len() >= MAX_HALF_SECONDS {
            return Err("Local key analysis supports up to 24 hours per file".into());
        }
        self.transform();
        let pitches = std::mem::take(&mut self.chroma);
        let tonal = std::mem::take(&mut self.tonal_frames);
        let sampled = std::mem::take(&mut self.sampled_frames);
        self.bins.push(
            (tonal > 0 && tonal * 5 >= sampled * 2).then(|| pitches.map(|x| x / tonal as f32)),
        );
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<(Vec<Option<Chroma>>, u64), String> {
        if self.frames == 0 {
            return Err("Audio has no complete frames".into());
        }
        let duration = (self.frames * 1000).div_ceil(u64::from(self.rate));
        if self.frames != (self.bins.len() as u64 * u64::from(self.rate)).div_ceil(2) {
            self.close_half()?;
        }
        Ok((self.bins, duration))
    }
}

/// A concentrated group of pitched peaks survives this gate; broadband percussion generally does not.
pub(crate) fn peak_chroma(
    power: &[f32],
    hz_per_bin: f32,
    minimum_concentration: f32,
) -> Option<[f32; 12]> {
    let first = (65. / hz_per_bin).ceil() as usize;
    let last = (2500. / hz_per_bin).floor() as usize;
    let total = power[first..=last].iter().sum::<f32>();
    let mut peaks = [(0.0, 0); 8];
    for i in first..=last {
        let p = power[i];
        if p <= peaks[7].0 || p <= power[i - 1] || p <= power[i + 1] {
            continue;
        }
        let mut j = 7;
        while j > 0 && p > peaks[j - 1].0 {
            peaks[j] = peaks[j - 1];
            j -= 1;
        }
        peaks[j] = (p, i);
    }
    if total <= 0. || peaks.iter().map(|p| p.0).sum::<f32>() < minimum_concentration * total {
        return None;
    }
    let mut chroma = [0.; 12];
    for (p, i) in peaks {
        if p <= 0. {
            break;
        }
        let [a, b, c] = [power[i - 1], p, power[i + 1]].map(|x| x.max(1e-20).ln());
        let d = 0.5 * (a - c) / (a - 2. * b + c).min(-1e-8);
        let hz = (i as f32 + d.clamp(-0.5, 0.5)) * hz_per_bin;
        let midi = (69. + 12. * (hz / 440.).log2()).round() as i32;
        chroma[midi.rem_euclid(12) as usize] += p.sqrt();
    }
    let sum = chroma.iter().sum::<f32>();
    (sum > 0.).then(|| chroma.map(|x| x / sum))
}
