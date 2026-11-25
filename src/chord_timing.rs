//! Source coordinates and the shared listening clock for a pair of Prepare passages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PairTiming {
    pub starts_ms: [u64; 2],
    pub length_ms: u64,
    pub offset_b_ms: i64,
    pub tempos: [f64; 2],
    pub pitches: [i32; 2],
}

impl PairTiming {
    pub fn validate(self) -> Result<(), String> {
        if !(1..=16_000).contains(&self.length_ms)
            || self.offset_b_ms.unsigned_abs() > self.length_ms
            || self.starts_ms.iter().any(|start| *start > 86_400_000)
        {
            return Err(
                "Choose up to 16 seconds, an offset within the window and starts within 24 hours"
                    .into(),
            );
        }
        for (rate, pitch) in self.tempos.into_iter().zip(self.pitches) {
            let stretch = rate / 2_f64.powf(f64::from(pitch) / 12.);
            if !(-12..=12).contains(&pitch)
                || !(0.5..=2.).contains(&rate)
                || !(0.5..=2.).contains(&stretch)
            {
                return Err("Tempo and pitch together exceed the 0.5–2× stretch range".into());
            }
        }
        Ok(())
    }

    /// Absolute source range needed for the visible mix interval [0, length).
    pub fn source_window(self, side: usize) -> Option<(u64, u64)> {
        let start = *self.starts_ms.get(side)? as f64;
        let rate = self.tempos[side];
        let offset = if side == 1 { self.offset_b_ms } else { 0 };
        let mix_start = offset.max(0) as u64;
        if mix_start >= self.length_ms {
            return None;
        }
        let begin = start + (mix_start as f64 - offset as f64) * rate;
        let end = start + (self.length_ms as f64 - offset as f64) * rate;
        Some((begin.floor() as u64, end.ceil() as u64))
    }

    /// Inverse projection, including source events starting before visible mix time zero.
    pub fn mix_ms(self, side: usize, source_ms: u64) -> f64 {
        let offset = if side == 1 { self.offset_b_ms } else { 0 };
        offset as f64 + (source_ms as f64 - self.starts_ms[side] as f64) / self.tempos[side]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d01_projection_uses_the_same_offsets_and_tempos_in_both_directions() {
        let mut pair = PairTiming {
            starts_ms: [1000, 4000],
            length_ms: 8000,
            offset_b_ms: 1000,
            tempos: [0.5, 1.5],
            pitches: [0, 0],
        };
        assert!(pair.validate().is_ok());
        assert_eq!(pair.source_window(0), Some((1000, 5000)));
        assert_eq!(pair.source_window(1), Some((4000, 14500)));
        assert_eq!(pair.mix_ms(1, 5500), 2000.);
        pair.offset_b_ms = -1000;
        assert_eq!(pair.source_window(1), Some((5500, 17500)));
        assert_eq!(pair.mix_ms(1, 5500), 0.);
        pair.offset_b_ms = 8000;
        assert_eq!(pair.source_window(1), None);
        pair.tempos[0] = f64::NAN;
        assert!(pair.validate().is_err());
        pair.tempos[0] = 2.;
        pair.pitches[0] = -12;
        assert!(pair.validate().is_err());
    }
}
