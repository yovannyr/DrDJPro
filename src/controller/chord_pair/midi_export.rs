use super::*;
use crate::{
    beatgrid::BeatGrid,
    chord_passage::midi_triad,
    midi_file::{self, Note, Timing},
};
const PPQ: u16 = 960;
pub(super) struct Request {
    pair: Arc<Snapshot>,
    side: usize,
    grid: Arc<BeatGrid>,
    db: ChordConfirmations,
    folder: LibraryFolderPicker,
}
fn timing(grid: BeatGrid, begin: u64, end: u64) -> Result<Timing, String> {
    let (bpm, numerator, denominator) = grid.midi_passage(begin, end)?;
    let end_tick = ticks(end - begin, bpm);
    if end_tick == 0 {
        return Err("The interval is shorter than one MIDI tick.".into());
    }
    Ok(Timing {
        ppq: PPQ,
        bpm,
        numerator,
        denominator,
        end_tick,
    })
}
fn ticks(ms: u64, bpm: f64) -> u32 {
    (ms as f64 * bpm * f64::from(PPQ) / 60000.).round() as u32
}
fn passage_notes(
    events: &[crate::chord_passage::ChordEvent],
    begin: u64,
    end: u64,
    t: Timing,
    c: &Cancellation,
) -> Result<Vec<Note>, String> {
    let mut notes = Vec::new();
    for e in events {
        c.check()?;
        if e.end_ms <= begin || e.start_ms >= end {
            continue;
        }
        let Some(label) = &e.chord else { continue };
        let pitches = midi_triad(label).ok_or("Invalid stored chord.")?;
        let i = ticks(e.start_ms.max(begin) - begin, t.bpm);
        let j = ticks(e.end_ms.min(end) - begin, t.bpm).min(t.end_tick);
        if j > i {
            for pitch in pitches {
                notes.push(Note {
                    start: i,
                    duration: j - i,
                    pitch,
                    velocity: 96,
                    channel: 0,
                });
            }
        }
    }
    Ok(notes)
}

impl Request {
    pub(super) fn run(self, c: &Cancellation) -> Result<Option<PathBuf>, String> {
        let l = &self.pair.layers[self.side];
        l.verify(c)?;
        let (begin, end) = self
            .pair
            .timing
            .source_window(self.side)
            .ok_or("No source passage in the selection.")?;
        let t = timing(*self.grid, begin, end)?;
        let Some(dir) = (self.folder)() else {
            return Ok(None);
        };
        c.check()?;
        l.verify(c)?;
        let events = self
            .db
            .lookup(l.pin.id, &l.pin.path, &l.profile.source_sha256, begin, end)?;
        let notes = passage_notes(&events, begin, end, t, c)?;
        let base = format!("confirmed-chords-{}-{begin}-{end}", l.pin.id);
        let output = (0..10000)
            .map(|i| {
                if i == 0 {
                    dir.join(format!("{base}.mid"))
                } else {
                    dir.join(format!("{base}-{i}.mid"))
                }
            })
            .find(|p| matches!(p.try_exists(), Ok(false)))
            .ok_or("Cannot allocate a new MIDI file in that folder.")?;
        l.verify(c)?;
        c.check()?;
        midi_file::write(&output, t, &notes, c)?;
        Ok(Some(output))
    }
}
impl AudioController {
    pub(super) fn export_chord_midi(&self, side: i32) {
        let (Some(ui), Ok(side @ 0..=1)) = (self.pair_ui(), usize::try_from(side)) else {
            return;
        };
        let mut state = self.chord_pair.borrow_mut();
        if state.running(Kind::Confirm) || state.running(Kind::Export) {
            return;
        }
        let Some(pair) = state.session.as_ref().and_then(|s| s.pair.clone()) else {
            return;
        };
        let l = &pair.layers[side];
        let Some(grid) = self
            .library
            .borrow()
            .track_by_id(l.pin.id)
            .and_then(|t| t.grid)
        else {
            ui.set_chord_pair_status("Save a grid for this source before MIDI export.".into());
            return;
        };
        let Some((start, end)) = pair.timing.source_window(side) else {
            ui.set_chord_pair_status("The selection contains no interval from this source.".into());
            return;
        };
        if let Err(e) = timing(grid, start, end) {
            ui.set_chord_pair_status(e.into());
            return;
        }
        let Some(db) = self
            .persistence
            .borrow()
            .as_ref()
            .map(|w| w.chord_confirmations())
        else {
            ui.set_chord_pair_status("Library unavailable for confirmed chords.".into());
            return;
        };
        let Some(folder) = self.library_folder_picker.borrow().clone() else {
            ui.set_chord_pair_status("No destination folder picker available.".into());
            return;
        };
        state.cancel();
        let grid = Arc::new(grid);
        state.export_guard = Some((l.pin.id, grid.clone()));
        state.pending = Some(Work::Export(Request {
            pair,
            side,
            grid,
            db,
            folder,
        }));
        ui.set_chord_pair_status("Choose a folder for the confirmed source chords.".into());
        drop(state);
        self.poll_chord_pair();
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    use crate::chord_passage::ChordEvent;

    #[test]
    fn d02_clips_confirmed_chords_and_preserves_unconfirmed_rests() {
        let event = |start_ms, end_ms, chord: Option<&str>| ChordEvent {
            start_ms,
            end_ms,
            chord: chord.map(str::to_owned),
            confidence: 100,
        };
        let events = [
            event(0, 100, Some("F:maj")),
            event(200, 1000, Some("C:maj")),
            event(1000, 4500, None),
            event(4500, 9500, Some("G:maj")),
        ];
        let t = Timing {
            ppq: PPQ,
            bpm: 120.,
            numerator: 4,
            denominator: 4,
            end_tick: 16 * u32::from(PPQ),
        };
        let notes = passage_notes(&events, 500, 8500, t, &Cancellation::default()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("confirmed.mid");
        midi_file::write(&output, t, &notes, &Cancellation::default()).unwrap();
        midi_file::test_reader::assert_track(&output, t, &notes);
        let actual: Vec<_> = notes
            .iter()
            .map(|n| (n.pitch, n.start, n.duration))
            .collect();
        assert_eq!(
            actual,
            [
                (60, 0, 960),
                (64, 0, 960),
                (67, 0, 960),
                (67, 7680, 7680),
                (71, 7680, 7680),
                (74, 7680, 7680)
            ]
        );
    }
}
