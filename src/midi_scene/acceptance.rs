use super::*;
use crate::{
    midi_scene::transpose::{Proposal, Scale},
    scene_transcription::transcribe_mono,
};

fn event(role: Role, pitch: u8, start: u32) -> Event {
    Event {
        role,
        approved: true,
        uncertain: false,
        note: Note {
            pitch,
            start,
            duration: 96,
            velocity: 83,
            channel: match role {
                Role::Melody => 0,
                Role::Accompaniment => 1,
                Role::Pad(_) => 9,
            },
        },
    }
}

#[test]
fn m05_coupled_groove_and_r09_scale_edits_preserve_other_tracks() {
    let mut s = Scene {
        events: vec![
            event(Role::Melody, 60, 0),
            event(Role::Melody, 64, 240),
            event(Role::Melody, 67, 480),
            event(Role::Pad(0), 42, 240),
            event(Role::Pad(1), 46, 240),
            event(Role::Pad(2), 38, 480),
        ],
        groove: Groove {
            mask: 3,
            window_ms: 12,
            intensity: 0,
        },
        ..Scene::default()
    };
    assert!(s.rendered() == s.events);
    s.groove.intensity = 50;
    let grooved = s.rendered();
    assert_eq!(grooved[3].note.start, grooved[4].note.start);
    assert!((1..=11).contains(&grooved[3].note.start.abs_diff(240)));
    for i in [0, 1, 2, 5] {
        assert!(grooved[i] == s.events[i]);
    }
    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("groove.json");
    let c = Cancellation::default();
    s.save(&saved, &c).unwrap();
    assert!(Scene::read(&saved, &c).unwrap().rendered() == grooved);
    for (dst, expected) in [
        (Scale::new(0, true).unwrap(), [60, 63, 67]),
        (Scale::new(2, false).unwrap(), [62, 66, 69]),
    ] {
        let proposal = Proposal::new(&s, Scale::default(), dst).unwrap();
        let projected = proposal.result(&s).unwrap();
        let actual: Vec<_> = projected.events[..3].iter().map(|e| e.note.pitch).collect();
        assert_eq!(actual, expected);
        assert!(projected.events[3..] == s.events[3..]);
        assert!(projected.events.iter().zip(&s.events).all(|(a, b)| (
            a.note.start,
            a.note.duration,
            a.note.velocity
        ) == (
            b.note.start,
            b.note.duration,
            b.note.velocity
        )));
    }
    s.events.push(event(Role::Melody, 66, 1000));
    let mut decision = Proposal::new(&s, Scale::default(), Scale::new(0, true).unwrap()).unwrap();
    assert!(!decision.ready() && decision.result(&s).is_err());
    decision.resolve(0, 65).unwrap();
    assert_eq!(
        decision
            .result(&s)
            .unwrap()
            .events
            .last()
            .unwrap()
            .note
            .pitch,
        65
    );
    assert_eq!(s.events.last().unwrap().note.pitch, 66);
}

#[test]
fn m03_eight_notes_and_m04_chosen_accompaniment_roundtrip_as_midi() {
    let rate = 16_000;
    let pitches = [60u8, 62, 64, 65, 67, 69, 71, 72];
    let mut audio = Vec::new();
    for (i, pitch) in pitches.into_iter().enumerate() {
        let freq = 440. * 2f32.powf((f32::from(pitch) - 69.) / 12.);
        audio.extend(
            (0..4800).map(|n| 0.3 * (std::f32::consts::TAU * freq * n as f32 / rate as f32).sin()),
        );
        audio.resize(audio.len() + if i == 3 { 6400 } else { 1600 }, 0.);
    }
    let c = Cancellation::default();
    let timing = Scene::default().timing;
    let mut notes: Vec<_> = transcribe_mono(&audio, rate, timing.bpm, timing.ppq, &c)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), pitches);
    assert!(
        notes[4]
            .start
            .saturating_sub(notes[3].start + notes[3].duration)
            >= 300
    );
    assert!(
        transcribe_mono(&[0.; 4800], rate, timing.bpm, timing.ppq, &c)
            .unwrap()
            .is_empty()
    );
    notes[1].pitch = 61;
    notes[2].start += 3;
    notes[3].duration -= 5;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("track.mid");
    let variants = crate::scene_bridge::variants(&notes, timing, 0, false).unwrap();
    assert_eq!([variants[0].len(), variants[1].len()], [8, 12]);
    for selected in [&notes, &variants[0], &variants[1]] {
        crate::midi_file::write(&path, timing, selected, &c).unwrap();
        crate::midi_file::test_reader::assert_track(&path, timing, selected);
    }
    let complete = std::fs::read(&path).unwrap();
    c.request();
    assert!(crate::midi_file::write(&path, timing, &notes, &c).is_err());
    assert_eq!(complete, std::fs::read(path).unwrap());
}
