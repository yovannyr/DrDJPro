//! Fixed-memory estimate of perceived drive over 50 ms windows in the offline decoder.
//! The weights are heuristics. Bass share and rhythmic contrast are relative to level.
pub(super) struct TrackEnergy {
    window_frames: u32,
    frames: u32,
    power: f64,
    bass_power: f64,
    low_pass: f64,
    coefficient: f64,
    total_power: f64,
    total_bass: f64,
    changes: f64,
    previous: Option<f64>,
    windows: u32,
}

impl TrackEnergy {
    pub(super) fn new(rate: u32) -> Self {
        Self {
            window_frames: (rate / 20).max(1),
            coefficient: 1.0 - (-std::f64::consts::TAU * 160.0 / f64::from(rate.max(1))).exp(),
            frames: 0,
            power: 0.,
            bass_power: 0.,
            low_pass: 0.,
            total_power: 0.,
            total_bass: 0.,
            changes: 0.,
            previous: None,
            windows: 0,
        }
    }

    pub(super) fn push(&mut self, mono: f32, peak: f32) {
        self.low_pass += self.coefficient * (f64::from(mono) - self.low_pass);
        self.bass_power += self.low_pass * self.low_pass;
        self.power += f64::from(peak).powi(2);
        self.frames += 1;
        if self.frames == self.window_frames {
            self.finish_window();
        }
    }

    fn finish_window(&mut self) {
        let power = self.power / f64::from(self.frames);
        if let Some(previous) = self.previous {
            self.changes += (power - previous).abs();
        }
        self.previous = Some(power);
        self.total_power += power;
        self.total_bass += self.bass_power / f64::from(self.frames);
        self.windows += 1;
        self.frames = 0;
        self.power = 0.;
        self.bass_power = 0.;
    }

    pub(super) fn finish(mut self) -> Option<u8> {
        if self.frames > 0 {
            self.finish_window();
        }
        if self.windows == 0 {
            return None;
        }
        let mean = self.total_power / f64::from(self.windows);
        if mean < 1e-9 {
            return Some(1);
        }
        let level = ((10. * mean.log10() + 48.) / 44.).clamp(0., 1.);
        let bass = (self.total_bass / self.total_power).clamp(0., 1.);
        let pulse = (self.changes / self.total_power).clamp(0., 1.);
        Some(1 + (9. * (0.35 * level + 0.25 * bass + 0.4 * pulse)).round() as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(rate: u32, frequency: f64, rhythmic: bool, gain: f64) -> u8 {
        let mut meter = TrackEnergy::new(rate);
        for frame in 0..rate * 4 {
            let t = f64::from(frame) / f64::from(rate);
            let pulse = if rhythmic && (t * 5.).fract() >= 0.25 {
                0.05
            } else {
                1.
            };
            let sample = (gain * pulse * (std::f64::consts::TAU * t * frequency).sin()) as f32;
            meter.push(sample, sample.abs());
        }
        meter.finish().unwrap()
    }

    #[test]
    fn pulses_and_bass_increase_drive_at_multiple_rates() {
        for rate in [8_000, 44_100, 48_000] {
            let steady = score(rate, 1_000., false, 0.5);
            let rhythmic = score(rate, 80., true, 0.5);
            assert!(rhythmic >= steady + 3);
            assert!(rhythmic.abs_diff(score(rate, 80., true, 0.25)) <= 1);
        }
    }

    #[test]
    fn empty_silent_and_partial_windows() {
        assert_eq!(TrackEnergy::new(48_000).finish(), None);
        let mut meter = TrackEnergy::new(48_000);
        meter.push(0., 0.);
        assert_eq!(meter.finish(), Some(1));
    }
}
