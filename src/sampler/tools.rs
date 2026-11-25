use super::*;
use crate::{
    cancellation::Cancellation,
    source_monitor::analysis::{pitch, Pitch},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tuning {
    pub digest: [u8; 32],
    /// MIDI note of this one-shot after the confirmed correction.
    pub root: i32,
    pub correction: i32,
}
impl Tuning {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(0..=127).contains(&self.root) || !(-1200..=1200).contains(&self.correction) {
            return Err("Invalid sampler root or tuning correction".into());
        }
        Ok(())
    }
}
pub(super) fn pitch_ratio(cents: i32) -> Result<f32, String> {
    if !(-1200..=1200).contains(&cents) {
        return Err("Correction must be within one octave".into());
    }
    Ok(2_f32.powf(cents as f32 / 1200.))
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct Observation {
    pub digest: [u8; 32],
    pub pitch: Option<Pitch>,
}
#[derive(Debug)]
pub(crate) struct Loaded {
    pub path: PathBuf,
    pub observed: Observation,
    pub tuning: Option<Tuning>,
    pub pair: Option<LayerPair>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LayerMix {
    pub delay_ms: u16,
    pub level: u8,
    pub pan: i8,
}
impl LayerMix {
    pub fn new(delay: i32, level: i32, pan: i32) -> Result<Self, String> {
        if !(0..=5000).contains(&delay)
            || !(0..=100).contains(&level)
            || !(-100..=100).contains(&pan)
        {
            return Err("Choose delay 0–5000 ms, level 0–100% and pan −100 to +100".into());
        }
        Ok(Self {
            delay_ms: delay as u16,
            level: level as u8,
            pan: pan as i8,
        })
    }
}
impl Default for LayerMix {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            level: 100,
            pan: 0,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LayerPair {
    pub name: String,
    pub second_path: PathBuf,
    pub mixes: [LayerMix; 2],
}
impl LayerPair {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err("Name the combination (up to 80 bytes)".into());
        }
        if !self.second_path.is_absolute()
            || self
                .second_path
                .to_str()
                .is_none_or(|s| s.len() > 2048 || s.contains('\0'))
        {
            return Err("Invalid second layer path".into());
        }
        for mix in self.mixes {
            LayerMix::new(
                i32::from(mix.delay_ms),
                i32::from(mix.level),
                i32::from(mix.pan),
            )?;
        }
        Ok(())
    }
    pub(super) fn render(
        &self,
        first: Prepared,
        output_rate: u32,
        cancel: &Cancellation,
    ) -> Result<Prepared, String> {
        self.validate()?;
        let second = Prepared::open(&self.second_path, cancel)
            .map_err(|e| format!("Second layer {}: {e}", self.second_path.display()))?;
        let rate = first.rate.max(second.rate);
        if rate > output_rate {
            return Err(format!(
                "Sampler sample rate {rate} Hz exceeds output {output_rate} Hz"
            ));
        }
        let extent = |p: &Prepared, m: LayerMix| -> u64 {
            (u64::from(m.delay_ms) * u64::from(rate) + 500) / 1000
                + (p.samples.len() as u64 / 2 * u64::from(rate)).div_ceil(u64::from(p.rate))
        };
        let frames = extent(&first, self.mixes[0]).max(extent(&second, self.mixes[1]));
        if frames > MAX_FRAMES as u64 || frames > u64::from(rate) * 10 {
            return Err("Layer combination exceeds 10 seconds or 480000 frames".into());
        }
        let mut samples = vec![0.; frames as usize * 2];
        for (p, mix) in [(&first, self.mixes[0]), (&second, self.mixes[1])] {
            mix_layer(&mut samples, rate, &p.samples, p.rate, mix, cancel)?;
        }
        let peak = samples.iter().fold(1_f32, |a, v| a.max(v.abs()));
        if peak > 1. {
            for v in &mut samples {
                *v /= peak;
            }
        }
        Ok(Prepared {
            rate,
            path: first.path,
            samples,
        })
    }
}

fn mix_layer(
    out: &mut [f32],
    rate: u32,
    pcm: &[f32],
    src_rate: u32,
    m: LayerMix,
    cancel: &Cancellation,
) -> Result<(), String> {
    let offset = ((u64::from(m.delay_ms) * u64::from(rate) + 500) / 1000) as usize;
    let size = pcm.len() / 2;
    if size == 0 {
        return Err("Empty sampler layer".into());
    }
    let pan = f32::from(m.pan) / 100.;
    let gains = [1. - pan.max(0.), 1. + pan.min(0.)].map(|v| v * f32::from(m.level) / 100.);
    for (n, frame) in out[offset * 2..]
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .enumerate()
    {
        if n % 4096 == 0 {
            cancel.check()?;
        }
        let phase = n as u64 * u64::from(src_rate);
        let index = (phase / u64::from(rate)) as usize;
        if index >= size {
            break;
        }
        let fraction = (phase % u64::from(rate)) as f32 / rate as f32;
        let next = (index + 1).min(size - 1);
        for ch in 0..2 {
            let a = pcm[2 * index + ch];
            let b = pcm[2 * next + ch];
            frame[ch] += (a + (b - a) * fraction) * gains[ch];
        }
    }
    Ok(())
}

pub(super) fn read_sample(
    path: &Path,
    cancel: &Cancellation,
) -> Result<(Prepared, Observation), String> {
    let digest = crate::local_harmony::fingerprint(path, cancel)?;
    let sample = Prepared::open(path, cancel)?;
    if digest != crate::local_harmony::fingerprint(&sample.path, cancel)? {
        return Err("Sampler source changed while loading".into());
    }
    let pitch =
        strongest_window(&sample.samples, sample.rate).and_then(|(mono, rate)| pitch(&mono, rate));
    cancel.check()?;
    Ok((sample, Observation { digest, pitch }))
}

fn strongest_window(pcm: &[f32], rate: u32) -> Option<(Vec<f32>, f64)> {
    if !(8000..=192000).contains(&rate) || pcm.is_empty() {
        return None;
    }
    let frames = pcm.len() / 2;
    let width = (rate as usize / 4).min(frames);
    let stride = (rate / 12_000).max(1) as usize;
    if width < stride * 1024 {
        return None;
    }
    let hop = (width / 2).max(1);
    let mut best = (0_f64, 0, 0);
    for start in (0..=frames - width).step_by(hop) {
        let mut levels = [0_f64; 2];
        for pair in pcm[start * 2..(start + width) * 2].as_chunks::<2>().0 {
            for (sum, x) in levels.iter_mut().zip(pair) {
                *sum += f64::from(*x).powi(2);
            }
        }
        for (ch, level) in levels.into_iter().enumerate() {
            if level > best.0 {
                best = (level, start, ch);
            }
        }
    }
    if best.0 <= 1e-8 {
        return None;
    }
    let (_, start, ch) = best;
    let data = pcm[start * 2..(start + width) * 2]
        .chunks_exact(2 * stride)
        .take(4096)
        .map(|g| g.as_chunks::<2>().0.iter().map(|x| x[ch]).sum::<f32>() / stride as f32)
        .collect();
    Some((data, f64::from(rate) / stride as f64))
}
