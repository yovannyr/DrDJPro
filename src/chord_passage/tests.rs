use super::*;
use crate::test_support::{wav_samples, TempFile};
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
};

const RATE: u32 = 12_000;
const BAR: u64 = 2000; // 120 BPM, four beats per bar.
const FOUR: [(u8, bool); 4] = [(2, true), (7, true), (9, false), (2, true)];

fn fixture(chords: &[(u8, bool)], offset_ms: u32) -> TempFile {
    let mut pcm = vec![0i16; (u64::from(offset_ms) * u64::from(RATE) / 1000 * 2) as usize];
    let count = RATE as usize * 2;
    for (bar, &(root, minor)) in chords.iter().enumerate() {
        // The second bar uses the first inversion; its third is the lowest note.
        let pitches = [
            root + if bar == 1 { 12 } else { 0 },
            root + if minor { 3 } else { 4 },
            root + 7,
        ];
        let hz = pitches.map(|pitch| 220. * 2f64.powf((f64::from(pitch) - 9.) / 12.));
        for i in 0..count {
            let envelope = (i.min(count - 1 - i) as f64 / 50.).min(1.);
            let t = std::f64::consts::TAU * i as f64 / f64::from(RATE);
            let v = (hz
                .iter()
                .map(|f| (t * f).sin() + 0.08 * (2. * t * f).sin())
                .sum::<f64>()
                * envelope
                * 6100.) as i16;
            pcm.extend([v, -v]);
        }
    }
    wav_samples(&pcm, RATE, 2)
}

fn check_times(events: &[ChordEvent], bounds: &[u64], labels: &[&str]) {
    assert_eq!(events.len(), labels.len(), "{events:?}");
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.chord.as_deref(), Some(labels[i]), "{events:?}");
        assert!((1..=100).contains(&event.confidence));
        assert!(
            event.start_ms.abs_diff(bounds[i]) <= 125
                && event.end_ms.abs_diff(bounds[i + 1]) <= 125,
            "{events:?}"
        );
        if i > 0 {
            assert_eq!(events[i - 1].end_ms, event.start_ms);
        }
    }
}

#[test]
fn d01_held_and_changing_four_bar_chords_with_a_shifted_entry() {
    let cancel = Cancellation::default();
    let a = fixture(&[(2, true); 4], 0);
    let b = fixture(&FOUR, 173);
    let held = analyze(a.path(), 0, 4 * BAR, &cancel).unwrap();
    check_times(&held.events, &[0, 8000], &["D:min"]);
    let before = fingerprint(b.path(), &cancel).unwrap();
    let b_full = analyze(b.path(), 173, 4 * BAR, &cancel).unwrap();
    let target = [173, 2173, 4173, 6173, 8173];
    check_times(
        &b_full.events,
        &target,
        &["D:min", "G:min", "A:maj", "D:min"],
    );
    eprintln!(
        "D01 four-bar progression {:#?}; maximum boundary error={} ms",
        b_full.events,
        b_full
            .events
            .iter()
            .zip(target)
            .map(|(e, t)| e.start_ms.abs_diff(t))
            .max()
            .unwrap()
    );
    let shifted = analyze(b.path(), 2319, 10_000, &cancel).unwrap();
    check_times(
        &shifted.events,
        &[2319, 4173, 6173, 8173],
        &["G:min", "A:maj", "D:min"],
    );
    assert_eq!(shifted.events.first().unwrap().start_ms, 2319);
    assert_eq!(shifted.events.last().unwrap().end_ms, 8173);
    assert_eq!(shifted.source_sha256, before);
    assert_eq!(b_full.source_sha256, before);
}

#[test]
fn d01_unknown_noise_lone_notes_and_bounded_selection() {
    let mut state = 1u32;
    let pcm: Vec<i16> = (0..RATE * 4)
        .map(|i| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            if i < RATE * 2 {
                (state >> 16) as i16
            } else {
                (16_000. * (std::f64::consts::TAU * f64::from(i) * 440. / f64::from(RATE)).sin())
                    as i16
            }
        })
        .collect();
    let f = wav_samples(&pcm, RATE, 1);
    let cancel = Cancellation::default();
    let p = analyze(f.path(), 0, 5000, &cancel).unwrap();
    assert_eq!(
        p.events,
        vec![ChordEvent {
            start_ms: 0,
            end_ms: 4000,
            chord: None,
            confidence: 0
        }]
    );
    assert!(analyze(f.path(), 3999, 1, &cancel).is_ok());
    for (start, length) in [(4000, 50), (0, 0), (0, MAX_LENGTH_MS + 1), (u64::MAX, 2)] {
        assert!(analyze(f.path(), start, length, &cancel).is_err());
    }
    cancel.request();
    assert!(analyze(f.path(), 0, 4000, &cancel).is_err());
}

#[test]
fn d01_canonical_transposition_and_replacement_during_analysis() {
    assert_eq!(canonical_chord("F#m").as_deref(), Some("Gb:min"));
    assert_eq!(canonical_chord("c♭:maj").as_deref(), Some("B:maj"));
    assert_eq!(transpose_chord("Dm", -2).as_deref(), Some("C:min"));
    assert_eq!(transpose_chord("C:maj", i32::MIN).as_deref(), Some("E:maj"));
    for invalid in ["D:min7", "A:", "X", "F5", "D:dim", "N"] {
        assert!(canonical_chord(invalid).is_none());
    }
    let a = fixture(&[(2, true)], 0);
    let b = fixture(&[(7, true)], 0);
    let result = verified_profile(a.path(), &Cancellation::default(), || {
        let events = read_events(a.path(), 0, 2000, &Cancellation::default())?;
        fs::copy(b.path(), a.path()).unwrap();
        Ok(events)
    });
    assert!(result.unwrap_err().contains("changed"));
}

#[test]
#[ignore = "pre-registered local audio corpus, score annotations separately after freezing the detector"]
fn d01_corpus_intervals_tsv() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::var_os("DRDJPRO_HARMONY_CORPUS").ok_or("Set DRDJPRO_HARMONY_CORPUS")?,
    );
    let dest = PathBuf::from(std::env::var_os("DRDJPRO_CHORD_TSV").ok_or("Set DRDJPRO_CHORD_TSV")?);
    let mut dirs = vec![root.clone()];
    let mut paths = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                dirs.push(entry.path());
            } else if kind.is_file() && crate::audio_formats::supported(&entry.path()) {
                paths.push(entry.path());
            }
        }
    }
    if paths.is_empty() {
        return Err("No supported audio files in the chord corpus".into());
    }
    paths.sort();
    let temporary = tempfile::NamedTempFile::new_in(
        dest.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let mut out = BufWriter::new(temporary.as_file());
    writeln!(out, "path\tstart_ms\tend_ms\tchord\tconfidence")?;
    let cancel = Cancellation::default();
    for path in paths {
        let rel = path
            .strip_prefix(&root)?
            .components()
            .map(|c| c.as_os_str().to_str().ok_or("Non-UTF8 corpus path"))
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        let field = if rel.contains(['\t', '\n', '\r', '"']) {
            format!("\"{}\"", rel.replace('"', "\"\""))
        } else {
            rel
        };
        let p = analyze(&path, 0, MAX_LENGTH_MS, &cancel)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if p.events.last().is_some_and(|e| e.end_ms == MAX_LENGTH_MS) {
            return Err("Corpus recording reaches the 10-minute analysis limit".into());
        }
        for e in p.events {
            writeln!(
                out,
                "{field}\t{}\t{}\t{}\t{}",
                e.start_ms,
                e.end_ms,
                e.chord.as_deref().unwrap_or("unknown"),
                e.confidence
            )?;
        }
    }
    out.flush()?;
    drop(out);
    temporary.persist_noclobber(dest)?;
    Ok(())
}
