//! Source-time major/minor triad events. Analysis runs on the preparation worker.
mod detector;
#[cfg(test)]
mod tests;

use crate::{
    cancellation::Cancellation, library::import_review::Stamp, local_harmony::fingerprint,
    playback::clip_diagnosis::stream,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const ALGORITHM_VERSION: u16 = 1;
const MAX_LENGTH_MS: u64 = 600_000;
const NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChordEvent {
    pub start_ms: u64,
    pub end_ms: u64,
    pub chord: Option<String>,
    pub confidence: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChordProfile {
    pub source_sha256: [u8; 32],
    pub events: Vec<ChordEvent>,
}

fn name(code: u8) -> String {
    let root = NAMES[usize::from(code % 12)];
    let quality = if code < 12 { "maj" } else { "min" };
    format!("{root}:{quality}")
}

fn parse(value: &str) -> Option<(i32, bool)> {
    let mut chars = value.trim().chars();
    let natural = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let mut suffix = chars.as_str();
    let accidental = match suffix.chars().next() {
        Some(c @ ('#' | '♯' | 'b' | '♭')) => {
            suffix = &suffix[c.len_utf8()..];
            if matches!(c, '#' | '♯') {
                1
            } else {
                -1
            }
        }
        _ => 0,
    };
    let minor = match suffix {
        "" | "maj" | ":maj" => false,
        "m" | "min" | ":min" => true,
        _ => return None,
    };
    Some((natural + accidental, minor))
}

pub(crate) fn canonical_chord(value: &str) -> Option<String> {
    transpose_chord(value, 0)
}
pub(crate) fn midi_triad(value: &str) -> Option<[u8; 3]> {
    let (root, minor) = parse(value)?;
    let root = 60 + root.rem_euclid(12) as u8;
    Some([root, root + if minor { 3 } else { 4 }, root + 7])
}
pub(crate) fn transpose_chord(value: &str, semitones: i32) -> Option<String> {
    let (root, minor) = parse(value)?;
    let pc = (root + semitones.rem_euclid(12)).rem_euclid(12) as u8;
    Some(name(pc + if minor { 12 } else { 0 }))
}

fn frame_for_ms(ms: u64, rate: u32) -> Result<u64, String> {
    u64::try_from((u128::from(ms) * u128::from(rate)).div_ceil(1000))
        .map_err(|_| "Passage timestamp overflow".into())
}

pub(crate) fn analyze(
    path: &Path,
    start_ms: u64,
    length_ms: u64,
    cancel: &Cancellation,
) -> Result<ChordProfile, String> {
    cancel.check()?;
    if !(1..=MAX_LENGTH_MS).contains(&length_ms) {
        return Err("Chord analysis needs a passage of 1 ms to 10 minutes".into());
    }
    let end_ms = start_ms
        .checked_add(length_ms)
        .ok_or("Passage timestamp overflow")?;
    verified_profile(path, cancel, || read_events(path, start_ms, end_ms, cancel))
}

fn verified_profile(
    path: &Path,
    cancel: &Cancellation,
    read: impl FnOnce() -> Result<Vec<ChordEvent>, String>,
) -> Result<ChordProfile, String> {
    let stamp = Stamp::at(path).ok();
    let source_sha256 = fingerprint(path, cancel)?;
    let events = read()?;
    if source_sha256 != fingerprint(path, cancel)? || stamp != Stamp::at(path).ok() {
        return Err("Source changed during chord passage analysis".into());
    }
    Ok(ChordProfile {
        source_sha256,
        events,
    })
}

fn read_events(
    path: &Path,
    start_ms: u64,
    end_ms: u64,
    cancel: &Cancellation,
) -> Result<Vec<ChordEvent>, String> {
    let mut recognizer = None;
    let mut cursor = 0u64;
    stream(path, cancel, |channels, rate, pcm| {
        let width = usize::from(channels);
        let next = cursor
            .checked_add((pcm.len() / width) as u64)
            .ok_or("Source frame duration overflow")?;
        let first = frame_for_ms(start_ms, rate)?.max(cursor);
        let finish = frame_for_ms(end_ms, rate)?;
        let last = finish.min(next);
        if first < last {
            let worker = recognizer.get_or_insert_with(|| detector::Detector::new(rate));
            worker.push(
                &pcm[(first - cursor) as usize * width..(last - cursor) as usize * width],
                width,
                cancel,
            )?;
        }
        cursor = next;
        Ok(next >= finish)
    })?;
    recognizer
        .ok_or("No source frames in the selected chord passage")?
        .finish(start_ms, end_ms, cursor, cancel)
}
