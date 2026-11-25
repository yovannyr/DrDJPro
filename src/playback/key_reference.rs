//! Concert-pitch headphone reference. All tuning and table construction happen before publication.
use super::{tempo_preview::Excerpt, PreviewClip};
use drdjpro_ui_spike::analysis::keys::{CamelotKey, KeyMode};
use std::sync::OnceLock;

const TABLE_BITS: u32 = 10;
const TABLE_LEN: usize = 1 << TABLE_BITS;

pub(super) struct ReferenceVoice {
    excerpt: Excerpt,
    table: &'static [f32; TABLE_LEN],
    phase: [u32; 3],
    steps: [u32; 3],
    notes: usize,
    gain: f32,
    fade_frames: u64,
}

impl ReferenceVoice {
    pub(super) fn new(
        clip: &PreviewClip,
        key: CamelotKey,
        triad: bool,
        rate: u32,
    ) -> Result<Self, String> {
        let excerpt = Excerpt::new(clip, rate)?;
        if !(1..=12).contains(&key.number) {
            return Err("Invalid Camelot reference key".into());
        }
        // One clockwise Camelot step adds a fifth; 8B is C and its relative minor 8A is A.
        let minor = key.mode == KeyMode::Minor;
        let root =
            60 + ((i32::from(key.number) - 8) * 7 - if minor { 3 } else { 0 }).rem_euclid(12);
        let intervals = [0, if minor { 3 } else { 4 }, 7];
        let notes = if triad { 3 } else { 1 };
        let mut steps = [0; 3];
        for (step, offset) in steps[..notes].iter_mut().zip(intervals) {
            let hz = 440.0 * 2.0_f64.powf(f64::from(root + offset - 69) / 12.0);
            if hz >= f64::from(rate) * 0.5 {
                return Err("Output rate too low for the selected reference notes".into());
            }
            *step = (hz * 4_294_967_296.0 / f64::from(rate)).round() as u32;
        }
        static WAVE: OnceLock<[f32; TABLE_LEN]> = OnceLock::new();
        let table = WAVE.get_or_init(|| {
            std::array::from_fn(|i| (std::f32::consts::TAU * i as f32 / TABLE_LEN as f32).sin())
        });
        // A five-millisecond edge envelope avoids discontinuities at arbitrary excerpt boundaries.
        let fade_frames = (u64::from(rate) / 200).min((excerpt.frames - 1) / 2);
        Ok(Self {
            excerpt,
            table,
            phase: [0; 3],
            steps,
            notes,
            gain: 0.18 / notes as f32,
            fade_frames,
        })
    }

    pub(super) fn frame(&mut self) -> [f32; 2] {
        if self.excerpt.cursor == self.excerpt.frames {
            self.excerpt.cursor = 0;
        }
        let cursor = self.excerpt.cursor;
        let distance = cursor.min(self.excerpt.frames - 1 - cursor);
        let envelope = if distance < self.fade_frames {
            distance as f32 / self.fade_frames as f32
        } else {
            1.0
        };
        let passage = self.excerpt.frame().unwrap_or_default();
        let mut tone = 0.;
        for i in 0..self.notes {
            tone += self.table[(self.phase[i] >> (32 - TABLE_BITS)) as usize];
            self.phase[i] = self.phase[i].wrapping_add(self.steps[i]);
        }
        let tone = tone * self.gain;
        [
            (passage[0] + tone) * envelope,
            (passage[1] + tone) * envelope,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::Cancellation;
    use std::time::Duration;

    #[test]
    fn a07_all_camelot_keys_tune_the_tonic_third_and_fifth() {
        let file = crate::test_support::wav_file(0, 64);
        let clip =
            PreviewClip::open(file.path(), Duration::ZERO, &Cancellation::default()).unwrap();
        // Pitch classes (C = 0) in published Camelot-number order, independent of the tuning code.
        for (mode, tonics, third) in [
            (KeyMode::Major, [11, 6, 1, 8, 3, 10, 5, 0, 7, 2, 9, 4], 4),
            (KeyMode::Minor, [8, 3, 10, 5, 0, 7, 2, 9, 4, 11, 6, 1], 3),
        ] {
            for (index, pitch_class) in tonics.into_iter().enumerate() {
                let key = CamelotKey {
                    number: index as u8 + 1,
                    mode,
                };
                let voice = ReferenceVoice::new(&clip, key, true, 44_100).unwrap();
                for (step, interval) in voice.steps.into_iter().zip([0, third, 7]) {
                    let measured = f64::from(step) * 44_100. / 4_294_967_296.;
                    let expected = 440. * 2.0_f64.powf(f64::from(pitch_class + interval - 9) / 12.);
                    assert!(
                        (measured - expected).abs() < 0.00001,
                        "{key} interval {interval}"
                    );
                }
            }
        }
    }
}
