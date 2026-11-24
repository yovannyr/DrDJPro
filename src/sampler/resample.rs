//! Band-limited fractional reads for upward pitch shifts. Tables are initialized before
//! constructing the audio bus and shared across all voices, previews and sampler banks.
use super::Bank;
use std::sync::OnceLock;

const TAPS: usize = 32;
const PHASES: usize = 64;
const BANDS: usize = 32;
pub(super) struct Filters(Box<[[f32; TAPS]]>);

pub(super) fn filters() -> &'static Filters {
    static TABLE: OnceLock<Filters> = OnceLock::new();
    TABLE.get_or_init(|| {
        let rows = (0..BANDS * PHASES)
            .map(|row| {
                let band = row / PHASES + 1;
                let phase = (row % PHASES) as f64 / PHASES as f64;
                let bandwidth = 0.94 * band as f64 / BANDS as f64;
                let mut taps = std::array::from_fn(|i| {
                    let t = i as f64 - (TAPS / 2 - 1) as f64 - phase;
                    let a = std::f64::consts::PI * bandwidth * t;
                    let sinc = if a.abs() < 1e-9 { 1.0 } else { a.sin() / a };
                    let w = std::f64::consts::TAU * i as f64 / (TAPS - 1) as f64;
                    (sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos())) as f32
                });
                let dc: f32 = taps.iter().sum();
                for tap in &mut taps {
                    *tap /= dc;
                }
                taps
            })
            .collect();
        Filters(rows)
    })
}

/// Zero means no decimation. A nonzero band rounds the anti-alias cutoff down.
pub(super) fn band(increment: u64, rate: u32) -> usize {
    if increment <= u64::from(rate) {
        0
    } else {
        ((u64::from(rate) * BANDS as u64 / increment) as usize).clamp(1, BANDS)
    }
}
impl Filters {
    pub(super) fn stereo(
        &self,
        bank: &Bank,
        frames: usize,
        frame: usize,
        fraction: f32,
        band: usize,
    ) -> [f32; 2] {
        if band == 0 {
            let next = (frame + 1).min(frames - 1);
            let [left, right] = bank.pcm[frame].load();
            let [next_left, next_right] = bank.pcm[next].load();
            return [
                left + (next_left - left) * fraction,
                right + (next_right - right) * fraction,
            ];
        }
        let phase = ((fraction * PHASES as f32) as usize).min(PHASES - 1);
        let weights = &self.0[(band - 1) * PHASES + phase];
        let mut out = [0.; 2];
        // The full interior window is contiguous. Only the two clip edges need tap clamping.
        if let Some(first) = frame.checked_sub(TAPS / 2 - 1) {
            let end = first + TAPS;
            if end <= frames {
                for (&weight, pcm) in weights.iter().zip(&bank.pcm[first..end]) {
                    let [left, right] = pcm.load();
                    out[0] += weight * left;
                    out[1] += weight * right;
                }
                return out;
            }
        }
        for (i, &w) in weights.iter().enumerate() {
            let j = (frame + i).saturating_sub(TAPS / 2 - 1).min(frames - 1);
            let [left, right] = bank.pcm[j].load();
            out[0] += w * left;
            out[1] += w * right;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_fir_matches_scalar_reference_at_clip_edges_and_interior() {
        let bank = Bank::new().unwrap();
        let reference: [f32; 160] = std::array::from_fn(|i| ((i * 17) % 47) as f32 / 47. - 0.5);
        for (to, &from) in bank.pcm.iter().zip(reference.as_chunks::<2>().0) {
            to.store(from);
        }
        let filters = filters();
        for frames in [1, TAPS - 1, TAPS, TAPS + 1, 80] {
            for frame in 0..frames {
                let next = (frame + 1).min(frames - 1);
                for fraction in [0., 0.125, 0.75] {
                    let expected: [f32; 2] = std::array::from_fn(|ch| {
                        let a = reference[2 * frame + ch];
                        a + (reference[2 * next + ch] - a) * fraction
                    });
                    let actual = filters.stereo(&bank, frames, frame, fraction, 0);
                    assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
                }
                for band in [1, BANDS / 2, BANDS] {
                    for phase in [0, PHASES / 2, PHASES - 1] {
                        let mut expected = [0_f32; 2];
                        for (tap, &weight) in
                            filters.0[(band - 1) * PHASES + phase].iter().enumerate()
                        {
                            let idx = (frame + tap).saturating_sub(TAPS / 2 - 1).min(frames - 1);
                            for (ch, sum) in expected.iter_mut().enumerate() {
                                *sum += weight * reference[2 * idx + ch];
                            }
                        }
                        let actual = filters.stereo(
                            &bank,
                            frames,
                            frame,
                            phase as f32 / PHASES as f32,
                            band,
                        );
                        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
                    }
                }
            }
        }
    }
}
