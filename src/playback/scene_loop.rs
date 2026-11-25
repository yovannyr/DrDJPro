//! Positions are shared by all three pre-rendered stems, including muted stems.
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

#[derive(Clone)]
pub(crate) struct SceneLoopControl(Arc<AtomicU8>);
impl SceneLoopControl {
    pub(crate) fn select(&self, variant: u8) -> Result<(), String> {
        if variant > 2 {
            return Err("Choose original, bass or chords.".into());
        }
        self.0.store(variant, Ordering::Release);
        Ok(())
    }
}
pub(super) struct Voice {
    stems: [Arc<Vec<f32>>; 3],
    selector: SceneLoopControl,
    active: u8,
    levels: [f32; 2],
    steps: [f32; 2],
    remaining: u32,
    ramp: u32,
    time: u64,
    period: u64,
    rate: u64,
    out_rate: u64,
}
impl Voice {
    pub(super) fn new(
        original: Arc<Vec<f32>>,
        layers: [Arc<Vec<f32>>; 2],
        rate: u32,
        out_rate: u32,
        variant: u8,
    ) -> Result<(Self, SceneLoopControl), String> {
        let samples = original.len();
        if samples == 0
            || !samples.is_multiple_of(2)
            || layers.iter().any(|s| s.len() != samples)
            || samples > 64 * 1024 * 1024 / 12
            || !(8000..=192000).contains(&rate)
            || !(8000..=384000).contains(&out_rate)
            || variant > 2
        {
            return Err(
                "Prepare three equal stereo loops within the 64 MiB preview budget.".into(),
            );
        }
        let selector = SceneLoopControl(Arc::new(AtomicU8::new(variant)));
        let [bass, chords] = layers;
        Ok((
            Self {
                stems: [original, bass, chords],
                selector: selector.clone(),
                active: variant,
                levels: [(variant == 1) as u8 as f32, (variant == 2) as u8 as f32],
                steps: [0.; 2],
                remaining: 0,
                ramp: out_rate.div_ceil(200),
                time: 0,
                period: (samples / 2) as u64 * u64::from(out_rate),
                rate: rate.into(),
                out_rate: out_rate.into(),
            },
            selector,
        ))
    }
    pub(super) fn frame(&mut self) -> [f32; 2] {
        let variant = self.selector.0.load(Ordering::Acquire);
        if variant != self.active {
            self.active = variant;
            self.remaining = self.ramp;
            for i in 0..2 {
                let goal = (variant == i as u8 + 1) as u8 as f32;
                self.steps[i] = (goal - self.levels[i]) / self.ramp as f32;
            }
        }
        if self.remaining > 0 {
            self.remaining -= 1;
            for i in 0..2 {
                if self.remaining == 0 {
                    self.levels[i] = (variant == i as u8 + 1) as u8 as f32;
                } else {
                    self.levels[i] += self.steps[i];
                }
            }
        }
        let index = (self.time / self.out_rate) as usize * 2;
        let next = if index + 2 == self.stems[0].len() {
            0
        } else {
            index + 2
        };
        let fraction = (self.time % self.out_rate) as f32 / self.out_rate as f32;
        self.time = (self.time + self.rate) % self.period;
        let mut frame = [0.; 2];
        for (stem, gain) in self.stems.iter().zip([1., self.levels[0], self.levels[1]]) {
            if gain == 0. {
                continue;
            }
            for lane in 0..2 {
                let a = stem[index + lane];
                let v = a + (stem[next + lane] - a) * fraction;
                if v.is_finite() {
                    frame[lane] += 0.48 * gain * v;
                }
            }
        }
        frame.map(|s| s.clamp(-1., 1.))
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    #[test]
    fn m04_switch_and_mute_use_one_continuous_loop_clock() {
        let n = 127usize;
        let lane = |base: f32| {
            Arc::new(
                (0..n)
                    .flat_map(|i| [base + i as f32 / 2048., 0.])
                    .collect::<Vec<_>>(),
            )
        };
        let (mut v, control) =
            Voice::new(lane(0.05), [lane(0.15), lane(0.30)], 8000, 8000, 0).unwrap();
        let mut frame = 0usize;
        for variant in [0u8, 1, 2, 0] {
            control.select(variant).unwrap();
            for phase in 0..71 {
                let sample = v.frame();
                if phase >= 40 {
                    let x = (frame % n) as f32 / 2048.;
                    let expected = 0.48
                        * (0.05
                            + x
                            + match variant {
                                1 => 0.15 + x,
                                2 => 0.30 + x,
                                _ => 0.,
                            });
                    assert!((sample[0] - expected).abs() < 0.00001);
                    assert_eq!(sample[1], 0.);
                }
                frame += 1;
            }
        }
    }
}
