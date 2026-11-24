//! Harmonic suggestions over a read-only catalog snapshot. The live decks are independent.
use crate::{cancellation::Cancellation, library::PlanningCatalog};
use drdjpro_ui_spike::analysis::keys::CamelotKey;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_RESULTS: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlanningTrack {
    pub id: i32,
    pub name: String,
    pub path: PathBuf,
    pub bpm: Option<f64>,
    pub key: Option<CamelotKey>,
    pub entry: Option<Duration>,
    pub exit: Option<Duration>,
    pub avoid: bool,
    pub available: bool,
}
#[derive(Clone, Copy)]
pub(crate) struct PlanningRef<'a> {
    pub id: i32,
    pub name: &'a str,
    pub path: &'a Path,
    pub bpm: Option<f64>,
    pub key: Option<CamelotKey>,
    pub entry: Option<Duration>,
    pub exit: Option<Duration>,
    pub entry_bpm: Option<f64>,
    pub exit_bpm: Option<f64>,
    pub avoid: bool,
    pub available: bool,
    pub in_scope: bool,
}
impl PlanningRef<'_> {
    pub fn owned(self) -> PlanningTrack {
        PlanningTrack {
            id: self.id,
            name: self.name.to_owned(),
            path: self.path.to_owned(),
            bpm: self.bpm,
            key: self.key,
            entry: self.entry,
            exit: self.exit,
            avoid: self.avoid,
            available: self.available,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Variant {
    pub track: PlanningTrack,
    pub shift: i8,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Leg {
    pub score: u8,
    pub reason: String,
    pub from_exit: Option<Duration>,
    pub to_entry: Option<Duration>,
}
#[derive(Clone, Debug)]
pub(crate) struct Bridge {
    pub track: PlanningTrack,
    pub incoming: Leg,
    pub outgoing: Leg,
    pub shift: Option<i8>,
}

#[derive(Clone, Copy)]
struct Evidence {
    tempo: Option<(f64, f64)>,
    keys: Option<(CamelotKey, CamelotKey)>,
    score: u8,
}
fn harmonic_score(a: CamelotKey, b: CamelotKey) -> u8 {
    if a == b {
        100
    } else if a.number == b.number {
        90
    } else if a.mode == b.mode && matches!(a.number.abs_diff(b.number), 1 | 11) {
        85
    } else {
        25
    }
}
impl Evidence {
    fn assess(
        from: PlanningRef<'_>,
        from_shift: i8,
        to: PlanningRef<'_>,
        to_shift: i8,
        max_bpm: Option<f64>,
        require_key: bool,
    ) -> Option<Self> {
        let tempo = from.exit_bpm.zip(to.entry_bpm);
        let delta = tempo.map(|(a, b)| (a - b).abs());
        if max_bpm.is_some_and(|max| delta.is_none_or(|difference| difference > max)) {
            return None;
        }
        let keys = from
            .key
            .zip(to.key)
            .map(|(a, b)| (transpose(a, from_shift), transpose(b, to_shift)));
        let key = keys.map(|(a, b)| harmonic_score(a, b));
        if require_key && key.is_none_or(|score| score < 85) {
            return None;
        }
        // Each documented dimension contributes 50 points; an unknown dimension contributes zero.
        let tempo_points = delta.map_or(0, |d| (50. - 1.5 * d).clamp(0., 50.).round() as u8);
        Some(Self {
            tempo,
            keys,
            score: tempo_points + key.unwrap_or(0).div_ceil(2),
        })
    }
    fn describe(self, from: PlanningRef<'_>, to: PlanningRef<'_>) -> Leg {
        let bpm = self.tempo.map_or_else(
            || "BPM unknown".to_owned(),
            |(a, b)| format!("{a:.1} → {b:.1} BPM (Δ {:.1})", (a - b).abs()),
        );
        let key = self.keys.map_or_else(
            || "key unknown".to_owned(),
            |(a, b)| {
                let relation = match harmonic_score(a, b) {
                    100 => "same key",
                    90 => "relative keys",
                    85 => "adjacent keys",
                    _ => "distant keys",
                };
                format!("{a} → {b} ({relation})")
            },
        );
        Leg {
            score: self.score,
            reason: format!("{bpm}, {key}"),
            from_exit: from.exit,
            to_entry: to.entry,
        }
    }
}

fn transpose(key: CamelotKey, semitones: i8) -> CamelotKey {
    CamelotKey {
        number: (i16::from(key.number) - 1 + 7 * i16::from(semitones)).rem_euclid(12) as u8 + 1,
        mode: key.mode,
    }
}

/// A relative major or minor retains its mode and lands on the anchor's key signature.
fn matching_shift(anchor: CamelotKey, candidate: CamelotKey, limit: u8) -> Option<i8> {
    for distance in 0..=limit as i8 {
        if transpose(candidate, distance).number == anchor.number {
            return Some(distance);
        }
        if distance > 0 && transpose(candidate, -distance).number == anchor.number {
            return Some(-distance);
        }
    }
    None
}

/// Stable key for reproducible selection; independent of hash-map and library insertion order.
fn lottery(seed: u64, id: i32) -> u64 {
    let mut x = seed.wrapping_add(u64::from(id as u32));
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Keep at most the best 24 elements, even when the catalog has hundreds of thousands of tracks.
fn retain_best<K: Ord, V>(list: &mut Vec<(K, V)>, key: K, item: V) {
    let at = list
        .binary_search_by(|(prior, _)| prior.cmp(&key))
        .unwrap_or_else(|at| at);
    if at < MAX_RESULTS {
        if list.len() == MAX_RESULTS {
            list.pop();
        }
        list.insert(at, (key, item));
    }
}

impl PlanningCatalog {
    fn anchor(&self, id: i32, cancel: &Cancellation) -> Result<PlanningRef<'_>, String> {
        for (i, row) in self.rows().enumerate() {
            if i.is_multiple_of(256) {
                cancel.check()?;
            }
            if row.id == id {
                if !row.available || row.avoid {
                    return Err("Selected endpoint is unavailable or excluded for this gig".into());
                }
                return Ok(row);
            }
        }
        Err("Selected endpoint is no longer in the catalog".into())
    }

    pub(crate) fn variants(
        &self,
        anchor_id: i32,
        max_semitones: u8,
        seed: u64,
        cancel: &Cancellation,
    ) -> Result<Vec<Variant>, String> {
        if max_semitones > 6 {
            return Err("Choose a shift limit from 0 to 6 semitones".into());
        }
        let anchor = self
            .anchor(anchor_id, cancel)?
            .key
            .ok_or("The anchor has no known key")?;
        let mut groups: [Vec<((u64, i32), PlanningRef<'_>)>; 13] =
            std::array::from_fn(|_| Vec::new());
        for (i, track) in self.rows().enumerate() {
            if i.is_multiple_of(256) {
                cancel.check()?;
            }
            if track.id == anchor_id || !track.available || track.avoid || !track.in_scope {
                continue;
            }
            let Some(shift) = track
                .key
                .and_then(|key| matching_shift(anchor, key, max_semitones))
            else {
                continue;
            };
            retain_best(
                &mut groups[(shift + 6) as usize],
                (lottery(seed, track.id), track.id),
                track,
            );
        }
        let mut order: Vec<i8> = (-6..=6)
            .filter(|n| !groups[(n + 6) as usize].is_empty())
            .collect();
        order.sort_unstable_by_key(|&shift| (lottery(seed, 32 + i32::from(shift)), shift));
        let mut result = Vec::new();
        for round in 0..MAX_RESULTS {
            let before = result.len();
            for &shift in &order {
                if let Some((_, row)) = groups[(shift + 6) as usize].get(round) {
                    let key = row.key.expect("grouped candidates have known keys");
                    let heard = transpose(key, shift);
                    let relation = if heard.mode == anchor.mode {
                        "same key"
                    } else {
                        "relative key"
                    };
                    result.push(Variant {
                        track: row.owned(),
                        shift,
                        reason: format!(
                            "Original {key}, {shift:+} semitones → {heard}; {relation} of {anchor}"
                        ),
                    });
                    if result.len() == MAX_RESULTS {
                        return Ok(result);
                    }
                }
            }
            if result.len() == before {
                break;
            }
        }
        cancel.check()?;
        Ok(result)
    }

    pub(crate) fn bridges(
        &self,
        start: i32,
        goal: i32,
        max_bpm: Option<f64>,
        max_semitones: Option<u8>,
        cancel: &Cancellation,
    ) -> Result<Vec<Bridge>, String> {
        use std::cmp::Reverse;
        if start == goal {
            return Err("Choose different start and destination tracks".into());
        }
        if max_bpm.is_some_and(|n| !n.is_finite() || n < 0.) || max_semitones.is_some_and(|n| n > 6)
        {
            return Err("Choose a non-negative BPM difference and 0–6 semitones".into());
        }
        let a = self.anchor(start, cancel)?;
        let c = self.anchor(goal, cancel)?;
        if max_bpm.is_some() && (a.exit_bpm.is_none() || c.entry_bpm.is_none()) {
            return Err("Bridge unproven: an endpoint lacks BPM for the active tempo limit".into());
        }
        if max_semitones.is_some() && (a.key.is_none() || c.key.is_none()) {
            return Err(
                "Bridge unproven: an endpoint lacks a key for the active harmonic limit".into(),
            );
        }
        let limit = max_semitones.unwrap_or(0) as i8;
        let mut retained = Vec::new();
        for (i, b) in self.rows().enumerate() {
            if i.is_multiple_of(256) {
                cancel.check()?;
            }
            if b.id == start || b.id == goal || b.avoid || !b.in_scope || !b.available {
                continue;
            }
            let mut best = None;
            for shift in -limit..=limit {
                let Some(ab) = Evidence::assess(a, 0, b, shift, max_bpm, max_semitones.is_some())
                else {
                    continue;
                };
                let Some(bc) = Evidence::assess(b, shift, c, 0, max_bpm, max_semitones.is_some())
                else {
                    continue;
                };
                // Prefer the strongest weaker leg, then both legs together, then a smaller shift.
                let rank = (
                    Reverse(ab.score.min(bc.score)),
                    Reverse(u16::from(ab.score) + u16::from(bc.score)),
                    shift.unsigned_abs(),
                    shift,
                );
                if best.as_ref().is_none_or(|(prior, _, _)| rank < *prior) {
                    best = Some((rank, ab, bc));
                }
            }
            if let Some((rank, ab, bc)) = best {
                retain_best(&mut retained, (rank, b.id), (b, rank.3, ab, bc));
            }
        }
        cancel.check()?;
        Ok(retained
            .into_iter()
            .map(|(_, (b, shift, ab, bc))| Bridge {
                track: b.owned(),
                incoming: ab.describe(a, b),
                outgoing: bc.describe(b, c),
                shift: (ab.keys.is_some() && bc.keys.is_some()).then_some(shift),
            })
            .collect())
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    use crate::{
        beatgrid::BeatGrid, hot_cues::HotCue, library::Library, track_metadata::MusicLabels,
    };
    fn library(values: &[(&str, Option<f64>)]) -> (Library, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut lib = Library::default();
        for (i, &(key, bpm)) in values.iter().enumerate() {
            let path = dir.path().join(format!("track-{i}.wav"));
            lib.insert([path.clone()]);
            let id = lib.track_id_for_path(&path).unwrap();
            lib.set_music_labels(id, MusicLabels::parse(key, "").unwrap())
                .unwrap();
            if let Some(bpm) = bpm {
                lib.set_grid(&path, Some(BeatGrid::new(bpm, Duration::ZERO).unwrap()))
                    .unwrap();
            }
            for (slot, name, ms) in [(0, "intro", 1000), (1, "outro", 6000)] {
                lib.set_hot_cue(
                    &path,
                    slot,
                    Some(
                        HotCue::new(
                            Duration::from_millis(ms + i as u64),
                            "".to_owned() + name,
                            0,
                        )
                        .unwrap(),
                    ),
                )
                .unwrap();
            }
        }
        (lib, dir)
    }
    #[test]
    fn a08_seeded_real_catalog_respects_shift_limits_and_unknown_key() {
        let (lib, _dir) = library(&[
            ("8A", None),
            ("8A", None),
            ("1A", None),
            ("3A", None),
            ("6A", None),
            ("10A", None),
            ("", None),
            ("8B", None),
        ]);
        let catalog = lib.planning_catalog();
        let c = Cancellation::default();
        let variants = |limit, seed| {
            catalog
                .variants(1, limit, seed, &c)
                .unwrap()
                .into_iter()
                .map(|v| (v.track.id, v.shift))
                .collect::<Vec<_>>()
        };
        let zero = variants(0, 4);
        assert_eq!(zero.len(), 2);
        assert!(zero.iter().all(|(_, s)| *s == 0));
        let one = variants(1, 4);
        assert_eq!(one.len(), 4);
        assert!(one.iter().all(|(_, s)| s.abs() <= 1));
        let two = variants(2, 4);
        assert_eq!(two.len(), 6);
        assert!(two.iter().any(|(_, s)| *s == 2) && two.iter().any(|(_, s)| *s == -2));
        assert!(!two.iter().any(|(id, _)| *id == 1 || *id == 7));
        assert_eq!(two, variants(2, 4));
        assert!((5..20).any(|seed| variants(2, seed) != two));
    }
    #[test]
    fn r03_bridge_must_satisfy_both_tempo_legs_and_preserve_cue_times() {
        let (lib, _dir) = library(&[
            ("8A", Some(100.)),
            ("8A", Some(101.)),
            ("8A", Some(106.)),
            ("8A", Some(112.)),
            ("8A", None),
        ]);
        let c = Cancellation::default();
        let matches = lib
            .planning_catalog()
            .bridges(1, 4, Some(6.), Some(0), &c)
            .unwrap();
        assert_eq!(matches.len(), 1);
        let b = &matches[0];
        assert_eq!(b.track.id, 3);
        assert_eq!(
            (b.incoming.from_exit, b.incoming.to_entry),
            (
                Some(Duration::from_millis(6000)),
                Some(Duration::from_millis(1002))
            )
        );
        assert_eq!(
            (b.outgoing.from_exit, b.outgoing.to_entry),
            (
                Some(Duration::from_millis(6002)),
                Some(Duration::from_millis(1003))
            )
        );
    }
}
