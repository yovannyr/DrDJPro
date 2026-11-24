//! Prepare two accompaniment drafts from the approved melody. Rhythm is expressed only in
//! project ticks so the four-bar end point is independent of the final note-off.
use crate::midi_file::{Note, Timing};

fn triad(weights: &[u64; 12], tonic: u8, minor: bool) -> [u8; 3] {
    let scale = if minor {
        [0, 2, 3, 5, 7, 8, 10]
    } else {
        [0, 2, 4, 5, 7, 9, 11]
    };
    let mut selected = [tonic; 3];
    let mut best = None;
    for degree in [0, 4, 3, 5, 1, 2, 6] {
        let chord = [degree, (degree + 2) % 7, (degree + 4) % 7].map(|d| (tonic + scale[d]) % 12);
        let score = 3 * weights[chord[0] as usize]
            + 2 * weights[chord[1] as usize]
            + 2 * weights[chord[2] as usize];
        if best.is_none_or(|old| score > old) {
            best = Some(score);
            selected = chord;
        }
    }
    selected
}

/// The first variant is a two-hit bass line, the second a sustained diatonic chord per bar.
/// Use only pitched, approved input notes from the chosen melody role.
pub(crate) fn variants(
    melody: &[Note],
    timing: Timing,
    tonic: u8,
    minor: bool,
) -> Result<[Vec<Note>; 2], String> {
    if tonic >= 12
        || timing.ppq == 0
        || timing.numerator == 0
        || !timing.denominator.is_power_of_two()
        || !(20.0..=400.).contains(&timing.bpm)
        || melody.is_empty()
        || melody.len() > 65_536
    {
        return Err("Confirm the key, tempo and a melody for the four-bar loop.".into());
    }
    let span = u32::from(timing.ppq) * 4 * u32::from(timing.numerator);
    if !span.is_multiple_of(u32::from(timing.denominator)) {
        return Err("Time signature cannot be represented at this tick resolution.".into());
    }
    let bar = span / u32::from(timing.denominator);
    if bar < 4 || bar * 4 != timing.end_tick {
        return Err("Choose exactly four bars for the accompaniment.".into());
    }
    let mut weights = [[0u64; 12]; 4];
    let mut any = false;
    for note in melody {
        if note.pitch > 127
            || note.channel >= 16
            || note.duration == 0
            || note.velocity == 0
            || note.velocity > 127
        {
            return Err("Invalid melody note.".into());
        }
        if note.channel == 9 {
            continue;
        }
        let end = note
            .start
            .saturating_add(note.duration)
            .min(timing.end_tick);
        for (i, hist) in weights.iter_mut().enumerate() {
            let start = bar * i as u32;
            let duration = (start + bar).min(end).saturating_sub(start.max(note.start));
            hist[(note.pitch % 12) as usize] += u64::from(duration) * u64::from(note.velocity);
            any |= duration > 0;
        }
    }
    if !any {
        return Err("There are no melody notes inside the four-bar interval.".into());
    }
    let mut bass = Vec::with_capacity(8);
    let mut chords = Vec::with_capacity(12);
    for (i, hist) in weights.iter().enumerate() {
        let pitches = triad(hist, tonic, minor);
        let start = i as u32 * bar;
        for (j, pc) in [pitches[0], pitches[2]].into_iter().enumerate() {
            bass.push(Note {
                start: start + j as u32 * (bar / 2),
                duration: bar * 3 / 8,
                pitch: 36 + pc,
                velocity: if j == 0 { 90 } else { 72 },
                channel: 1,
            });
        }
        let mut previous = 0;
        for pc in pitches {
            let mut pitch = 48 + pc;
            if pitch <= previous {
                pitch += 12;
            }
            previous = pitch;
            chords.push(Note {
                start,
                duration: bar - bar / 8,
                pitch,
                velocity: 66,
                channel: 1,
            });
        }
    }
    Ok([bass, chords])
}
