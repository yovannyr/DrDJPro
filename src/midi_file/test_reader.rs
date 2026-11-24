//! Test-only decoder for explicit SMF channel events and conductor metadata emitted by the app.
use super::{Note, Timing};
use std::{collections::BTreeMap, path::Path};

fn number(data: &[u8], at: &mut usize) -> u32 {
    let mut value = 0;
    for _ in 0..4 {
        let b = data[*at];
        *at += 1;
        value = (value << 7) | u32::from(b & 127);
        if b & 128 == 0 {
            return value;
        }
    }
    panic!("Invalid variable-length quantity");
}
pub(crate) fn assert_track(path: &Path, timing: Timing, expected: &[Note]) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..12], b"MThd\0\0\0\x06\0\0\0\x01");
    assert_eq!(
        u16::from_be_bytes(bytes[12..14].try_into().unwrap()),
        timing.ppq
    );
    assert_eq!(&bytes[14..18], b"MTrk");
    assert_eq!(
        u32::from_be_bytes(bytes[18..22].try_into().unwrap()) as usize,
        bytes.len() - 22
    );
    let (mut i, mut tick) = (22, 0);
    let mut open = BTreeMap::new();
    let mut notes = Vec::new();
    let (mut tempo, mut meter, mut end) = (None, None, None);
    while i < bytes.len() {
        tick += number(&bytes, &mut i);
        let status = bytes[i];
        i += 1;
        if status == 0xff {
            let kind = bytes[i];
            i += 1;
            let len = number(&bytes, &mut i) as usize;
            let body = &bytes[i..i + len];
            i += len;
            match kind {
                0x51 => {
                    assert_eq!(tick, 0);
                    assert_eq!(len, 3);
                    tempo = Some(u32::from_be_bytes([0, body[0], body[1], body[2]]));
                }
                0x58 => {
                    assert_eq!(tick, 0);
                    assert_eq!(len, 4);
                    meter = Some((body[0], 1u8 << body[1]));
                }
                0x2f => {
                    assert_eq!(len, 0);
                    assert_eq!(i, bytes.len());
                    end = Some(tick);
                }
                _ => panic!("Unexpected MIDI metadata"),
            }
        } else {
            let ch = status & 15;
            let pitch = bytes[i];
            let vel = bytes[i + 1];
            i += 2;
            match status >> 4 {
                9 if vel > 0 => assert!(open.insert((ch, pitch), (tick, vel)).is_none()),
                8 | 9 => {
                    let (start, velocity) = open.remove(&(ch, pitch)).unwrap();
                    notes.push(Note {
                        start,
                        duration: tick - start,
                        pitch,
                        velocity,
                        channel: ch,
                    });
                }
                _ => panic!("Unexpected MIDI status"),
            }
        }
    }
    assert!(open.is_empty());
    assert_eq!(tempo, Some((60_000_000. / timing.bpm).round() as u32));
    assert_eq!(meter, Some((timing.numerator, timing.denominator)));
    assert_eq!(end, Some(timing.end_tick));
    let key = |n: &Note| (n.start, n.channel, n.pitch, n.duration, n.velocity);
    notes.sort_by_key(key);
    let mut want = expected.to_vec();
    want.sort_by_key(key);
    assert_eq!(notes, want);
}
