//! Four independent voices may share one immutable prepared one-shot. The active mapping fits in one atomic word.
use super::*;
use crate::{cancellation::Cancellation, playback::PreviewClip};
use drdjpro_ui_spike::analysis::keys::{CamelotKey, KeyMode};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Layout {
    pub slot: u8,
    pub root: u8,
    pub digest: [u8; 32],
    pub deck: u8,
    pub key: String,
    pub notes: [u8; 4],
}
impl Layout {
    fn scale(root: u8, key: CamelotKey) -> [u8; 4] {
        let minor = key.mode == KeyMode::Minor;
        let tonic =
            ((i32::from(key.number) - 8) * 7 + if minor { 9 } else { 0 }).rem_euclid(12) as u8;
        // At most eleven octave placements. Ties keep the lower tonic.
        let first = (tonic..=122)
            .step_by(12)
            .min_by_key(|n| n.abs_diff(root))
            .unwrap_or(tonic);
        [
            first,
            first + 2,
            first + if minor { 3 } else { 4 },
            first + 5,
        ]
    }
    pub(crate) fn make(
        slot: u8,
        tuning: &tools::Tuning,
        deck: u8,
        key: CamelotKey,
    ) -> Result<Self, String> {
        tuning.validate()?;
        let root = tuning.root as u8;
        let notes = Self::scale(root, key);
        let result = Self {
            slot,
            root,
            digest: tuning.digest,
            deck,
            key: key.to_string(),
            notes,
        };
        result.validate()?;
        Ok(result)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.slot >= 4
            || self.deck > 1
            || self.root > 127
            || CamelotKey::parse(&self.key).is_none_or(|k| self.notes != Self::scale(self.root, k))
            || self
                .notes
                .iter()
                .any(|&n| n > 127 || n.abs_diff(self.root) > 12)
            || self.notes.windows(2).any(|w| w[1] <= w[0])
        {
            return Err("Invalid tonal pad layout".into());
        }
        Ok(())
    }
    fn word(&self) -> u64 {
        let mut n = 1 << 32 | u64::from(self.slot);
        for (i, &note) in self.notes.iter().enumerate() {
            n |= ((i32::from(note) - i32::from(self.root) + 12) as u64) << (2 + i * 5);
        }
        n
    }
}
pub(super) struct PadSource<'a> {
    pub(super) filters: &'static resample::Filters,
    pub(super) pcm: &'a Bank,
    pub(super) ratio: f32,
}
impl Shared {
    pub(super) fn pad_source(&self, slot: usize) -> PadSource<'_> {
        let word = self.tonal.load(Ordering::Acquire);
        let control = &self.banks[slot];
        if word == 0 {
            return PadSource {
                filters: self.filters,
                pcm: control,
                ratio: 1.,
            };
        }
        PadSource {
            filters: self.filters,
            pcm: &self.banks[(word & 3) as usize],
            ratio: self.semitones[((word >> (2 + slot * 5)) & 31) as usize],
        }
    }
    pub(crate) fn use_layout(&self, layout: Option<&Layout>) -> Result<(), String> {
        let word = match layout {
            Some(l) => {
                l.validate()?;
                if self.source_token(usize::from(l.slot)).is_none() {
                    return Err("Tonal pad sample unavailable".into());
                }
                l.word()
            }
            None => 0,
        };
        if self.tonal.load(Ordering::Acquire) != word {
            self.stop_all();
            self.tonal.store(word, Ordering::Release);
        }
        Ok(())
    }
    /// Audition the four real transposed samples as a short sequence on the private PFL route.
    pub(crate) fn scale_clip(
        &self,
        layout: &Layout,
        token: u64,
        cancel: &Cancellation,
    ) -> Result<PreviewClip, String> {
        layout.validate()?;
        let slot = usize::from(layout.slot);
        let b = &self.banks[slot];
        let sequence = b.sequence.load(Ordering::SeqCst);
        if !sequence.is_multiple_of(2) || self.source_token(slot) != Some(token) {
            return Err("Sampler source changed".into());
        }
        let rate = b.rate.load(Ordering::Acquire);
        let n = b.frames.load(Ordering::Acquire) as usize;
        let pitch = f64::from(f32::from_bits(b.pitch.load(Ordering::Acquire)));
        let play_rate = self.output_rate;
        let step = (play_rate as usize / 3).max(1);
        let mut out = vec![0.; step * 4 * 2];
        for (j, &note) in layout.notes.iter().enumerate() {
            let ratio = pitch * f64::from(rate) / f64::from(play_rate)
                * 2_f64.powf((f64::from(note) - f64::from(layout.root)) / 12.);
            let band = resample::band(
                (ratio * f64::from(play_rate)).round().max(1.) as u64,
                play_rate,
            );
            for i in 0..step {
                if i % 2048 == 0 {
                    cancel.check()?;
                }
                let pos = i as f64 * ratio;
                let k = pos.floor() as usize;
                if k >= n {
                    break;
                }
                let f = pos.fract() as f32;
                let gain = (i.min(step - 1 - i).min(128) as f32 / 128.) * 0.5;
                let at = (j * step + i) * 2;
                let frame = self.filters.stereo(b, n, k, f, band);
                out[at] = frame[0] * gain;
                out[at + 1] = frame[1] * gain;
            }
        }
        if b.sequence.load(Ordering::SeqCst) != sequence || self.source_token(slot) != Some(token) {
            return Err("Sampler source changed".into());
        }
        PreviewClip::pad_sequence(out, play_rate, cancel)
    }
}
