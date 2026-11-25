//! Standard MIDI File format 0. Explicit musical end time preserves trailing rests.
use crate::cancellation::Cancellation;
use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Note {
    pub start: u32,
    pub duration: u32,
    pub pitch: u8,
    pub velocity: u8,
    pub channel: u8,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Timing {
    pub ppq: u16,
    pub bpm: f64,
    pub numerator: u8,
    pub denominator: u8,
    pub end_tick: u32,
}

fn delta(track: &mut Vec<u8>, mut value: u32) -> Result<(), String> {
    if value > 0x0fff_ffff {
        return Err("Event delta exceeds the SMF range.".into());
    }
    let mut bytes = [0u8; 4];
    let mut i = 3;
    bytes[i] = (value & 127) as u8;
    value >>= 7;
    while value > 0 {
        i -= 1;
        bytes[i] = (value & 127) as u8 | 128;
        value >>= 7;
    }
    track.extend_from_slice(&bytes[i..]);
    Ok(())
}
pub(crate) fn write(
    path: &Path,
    timing: Timing,
    notes: &[Note],
    cancel: &Cancellation,
) -> Result<(), String> {
    cancel.check()?;
    if timing.ppq == 0
        || timing.ppq > 0x7fff
        || !(4.0..=1000.).contains(&timing.bpm)
        || timing.numerator == 0
        || !timing.denominator.is_power_of_two()
        || timing.end_tick == 0
        || timing.end_tick > 0x0fff_ffff
        || notes.len() > 65536
    {
        return Err("Invalid MIDI time base, length or note count.".into());
    }
    let tempo = (60_000_000.0 / timing.bpm).round() as u32;
    let mut track = vec![0, 0xff, 0x51, 3];
    track.extend_from_slice(&tempo.to_be_bytes()[1..]);
    track.extend_from_slice(&[
        0,
        0xff,
        0x58,
        4,
        timing.numerator,
        timing.denominator.trailing_zeros() as u8,
        24,
        8,
    ]);
    let mut events = Vec::with_capacity(2 * notes.len());
    for n in notes {
        let end = n
            .start
            .checked_add(n.duration)
            .ok_or("MIDI note overflow.")?;
        if n.duration == 0
            || end > timing.end_tick
            || n.pitch > 127
            || !(1..=127).contains(&n.velocity)
            || n.channel > 15
        {
            return Err("MIDI note is outside the scene or its channel range.".into());
        }
        events.push((n.start, true, n.channel, n.pitch, n.velocity));
        events.push((end, false, n.channel, n.pitch, 0u8));
    }
    events.sort_unstable();
    let mut cursor = 0;
    for (index, (tick, on, ch, pitch, vel)) in events.into_iter().enumerate() {
        if index % 512 == 0 {
            cancel.check()?;
        }
        delta(&mut track, tick - cursor)?;
        cursor = tick;
        track.extend_from_slice(&[(if on { 0x90 } else { 0x80 }) | ch, pitch, vel]);
    }
    delta(&mut track, timing.end_tick - cursor)?;
    track.extend_from_slice(&[0xff, 0x2f, 0]);
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().ok_or("No MIDI export folder.")?)
        .map_err(|e| e.to_string())?;
    let file = tmp.as_file_mut();
    file.write_all(b"MThd\0\0\0\x06\0\0\0\x01")
        .and_then(|()| file.write_all(&timing.ppq.to_be_bytes()))
        .and_then(|()| file.write_all(b"MTrk"))
        .and_then(|()| file.write_all(&(track.len() as u32).to_be_bytes()))
        .and_then(|()| file.write_all(&track))
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    cancel.check()?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_reader;
