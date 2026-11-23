//! An isolated comparison of two metric interpretations at one downbeat.
use super::BeatGrid;
use std::time::Duration;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TempoComparison {
    original: Option<BeatGrid>,
    bpm: f64,
    anchor: Duration,
}

impl TempoComparison {
    pub fn new(
        original: Option<BeatGrid>,
        seed_bpm: f64,
        anchor: Duration,
    ) -> Result<Self, String> {
        let bpm = original.as_ref().map_or(seed_bpm, |bg| bg.bpm());
        BeatGrid::new(bpm, anchor)?;
        Ok(Self {
            original,
            bpm,
            anchor,
        })
    }

    pub fn preview(&self, multiplier: f64) -> Result<BeatGrid, String> {
        if ![0.5, 1.0, 2.0].contains(&multiplier) {
            return Err("Multiplier must be 0.5, 1.0, or 2.0".to_string());
        }
        let mut grid = BeatGrid::new(self.bpm * multiplier, self.anchor)?;
        if let Some(orig) = &self.original {
            grid = grid.with_time_signature(orig.beats_per_bar(), orig.meter_denominator())?;
        }
        Ok(grid)
    }

    /// Two valid interpretations in slow-to-fast order, expressed as tempo multipliers.
    pub fn pair(&self) -> (f64, f64) {
        if self.bpm <= 120.0 {
            (1.0, 2.0)
        } else {
            (0.5, 1.0)
        }
    }

    pub fn blocked_reason(&self) -> Option<&'static str> {
        self.original.as_ref().and_then(Self::protection)
    }

    pub(crate) fn protection(grid: &BeatGrid) -> Option<&'static str> {
        if grid.is_locked() {
            return Some("Beatgrid is locked; unlock it in Prepare");
        }
        if !grid.meter_changes().is_empty() {
            return Some("Meter changes: preview only");
        }
        if !grid.tempo_changes().is_empty() {
            return Some("Tempo changes: preview only");
        }
        if !grid.warp_markers().is_empty() {
            return Some("Warp markers: preview only");
        }
        None
    }

    pub fn apply_candidate(
        &self,
        multiplier: f64,
        current: Option<BeatGrid>,
    ) -> Result<BeatGrid, String> {
        if current != self.original {
            return Err("Saved grid changed; reopen the tempo comparison".into());
        }
        if let Some(reason) = self.blocked_reason() {
            return Err(reason.into());
        }
        self.preview(multiplier)
    }

    pub fn original(&self) -> Option<BeatGrid> {
        self.original
    }
    pub fn bpm(&self) -> f64 {
        self.bpm
    }
    pub fn anchor(&self) -> Duration {
        self.anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_tempo_pair_shares_downbeat_and_resolves_both_reference_bars() {
        let anchor = Duration::from_secs(1);
        let original = BeatGrid::new(85., anchor).unwrap();
        let draft = TempoComparison::new(Some(original), 120., anchor).unwrap();
        assert_eq!(draft.pair(), (1., 2.));
        let end = Duration::from_secs(5);
        for (factor, next_beat, next_bar) in [(1., 81_882, 183_529), (2., 64_941, 115_765)] {
            let grid = draft.apply_candidate(factor, Some(original)).unwrap();
            assert_eq!(grid.anchor(), anchor);
            let lines = grid.lines(anchor, end);
            let first = lines.iter().find(|line| line.first).unwrap();
            assert!(first.downbeat);
            assert_eq!(first.percent, 0.);
            let frame =
                |percent: f32| 48_000 + (f64::from(percent) / 100. * 192_000.).round() as i64;
            assert!((frame(lines[1].percent) - next_beat).abs() <= 1);
            let bar = lines
                .iter()
                .find(|line| line.bar == 2 && line.downbeat)
                .unwrap();
            assert!((frame(bar.percent) - next_bar).abs() <= 1);
        }
        let draft = TempoComparison::new(None, 150., anchor).unwrap();
        assert_eq!(draft.pair(), (0.5, 1.));
        assert!(draft.preview(2.).is_err());
        assert!(draft.preview(f64::NAN).is_err());
    }

    #[test]
    fn prepare_tempo_preview_preserves_meter_and_rejects_stale_or_protected_grid() {
        let anchor = Duration::from_secs(1);
        let original = BeatGrid::new(85., anchor)
            .unwrap()
            .with_time_signature(3, 8)
            .unwrap();
        for protected in [
            original.with_locked(true),
            original.with_time_signature_change(2, 3, 4).unwrap(),
            original.with_tempo_change(4, 100.).unwrap(),
            original
                .with_warp_marker(4, Duration::from_secs(3))
                .unwrap(),
        ] {
            let draft = TempoComparison::new(Some(protected), 0., anchor).unwrap();
            let preview = draft.preview(2.).unwrap();
            assert_eq!(
                (preview.beats_per_bar(), preview.meter_denominator()),
                (3, 8)
            );
            assert!(draft.blocked_reason().is_some());
            assert!(draft.apply_candidate(2., Some(protected)).is_err());
        }
        let draft = TempoComparison::new(Some(original), 0., anchor).unwrap();
        assert!(draft
            .apply_candidate(2., Some(original.with_locked(true)))
            .is_err());
        assert!(draft.apply_candidate(2., None).is_err());
    }
}
