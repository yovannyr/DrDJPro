use super::*;
use crate::{
    playback::measure_two_decks,
    test_support::{wav_file, wav_samples, TempFile},
};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

const RATE: u32 = 12_000;
const BAR_MS: u64 = 2_000; // Four beats at 120 BPM

fn cadence(chords: &[(u8, bool)], stereo: bool) -> TempFile {
    let mut samples = Vec::new();
    let length = RATE as usize * 2;
    for &(root, minor) in chords {
        let frequencies = [0, if minor { 3 } else { 4 }, 7]
            .map(|d| 261.625565 * 2f64.powf(f64::from(root + d) / 12.));
        for i in 0..length {
            let taper = (i.min(length - 1 - i) as f64 / 90.).min(1.);
            let phase = std::f64::consts::TAU * i as f64 / f64::from(RATE);
            let value =
                (frequencies.iter().map(|f| (phase * f).sin()).sum::<f64>() * taper * 6200.) as i16;
            samples.push(value);
            if stereo {
                samples.push(-value);
            }
        }
    }
    wav_samples(&samples, RATE, if stereo { 2 } else { 1 })
}

const MODULATION: [(u8, bool); 8] = [
    (0, false),
    (5, false),
    (7, false),
    (0, false),
    (2, false),
    (7, false),
    (9, false),
    (2, false),
];

#[test]
fn a06_modulation_is_located_within_one_bar_and_survives_antiphase_stereo() {
    let file = cadence(&MODULATION, true);
    let cancel = Cancellation::default();
    let before = fingerprint(file.path(), &cancel).unwrap();
    let profile = analyze(file.path(), &cancel).unwrap();
    assert_eq!(profile.source_sha256, before);
    assert_eq!(profile.duration_ms, 8 * BAR_MS);
    let parts = &profile.segments;
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert_eq!(parts[0].key.as_deref(), Some("8B"));
    assert_eq!(parts[1].key.as_deref(), Some("10B"));
    assert!(
        parts[1].start_ms.abs_diff(4 * BAR_MS) <= BAR_MS,
        "{parts:?}"
    );
    assert_eq!(parts[0].end_ms, parts[1].start_ms);
    assert_eq!(fingerprint(file.path(), &cancel).unwrap(), before);
    eprintln!(
        "A06 synthetic C/D sections {parts:?}, boundary error={} ms",
        parts[1].start_ms.abs_diff(4 * BAR_MS)
    );
}

#[test]
fn a06_chord_changes_in_one_key_and_a_single_triad_do_not_create_a_modulation() {
    let cancel = Cancellation::default();
    for (chords, expected) in [
        (&MODULATION[..4], Some("8B")),
        (
            &[(9, true), (2, true), (4, false), (9, true)][..],
            Some("8A"),
        ),
        (&[(0, false); 4][..], None),
    ] {
        let file = cadence(chords, false);
        let profile = analyze(file.path(), &cancel).unwrap();
        assert_eq!(profile.segments.len(), 1, "{:?}", profile.segments);
        assert_eq!(profile.segments[0].key.as_deref(), expected);
    }
}

#[test]
fn a06_silence_and_broadband_noise_are_unknown() {
    let mut state = 1u32;
    let pcm: Vec<i16> = (0..RATE * 6)
        .map(|i| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            if i < RATE * 2 {
                0
            } else {
                ((state >> 16) as i16) / 2
            }
        })
        .collect();
    let file = wav_samples(&pcm, RATE, 1);
    let profile = analyze(file.path(), &Cancellation::default()).unwrap();
    assert_eq!(
        profile.segments,
        vec![KeySection {
            start_ms: 0,
            end_ms: 6000,
            key: None,
            confidence: 0
        }]
    );
    let tonal = cadence(&MODULATION[..4], false);
    let wav = fs::read(tonal.path()).unwrap();
    let mut joined = wav[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| i16::from_le_bytes(*s))
        .collect::<Vec<_>>();
    joined.extend_from_slice(&pcm);
    let original_length = RATE as usize * 8;
    joined.extend_from_within(..original_length);
    let combined = wav_samples(&joined, RATE, 1);
    let combined = analyze(combined.path(), &Cancellation::default()).unwrap();
    let labels = combined
        .segments
        .iter()
        .map(|s| (s.start_ms, s.end_ms, s.key.as_deref()))
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        [
            (0, 8000, Some("8B")),
            (8000, 14000, None),
            (14000, 22000, Some("8B"))
        ]
    );
}

#[test]
fn a06_profile_bounds_version_identity_and_cancellation() {
    let file = wav_file(0, 12000);
    let cancel = Cancellation::default();
    let profile = analyze(file.path(), &cancel).unwrap();
    let mut bad = profile.clone();
    bad.segments[0].end_ms -= 1;
    assert!(bad.validate().is_err());
    bad = profile.clone();
    bad.segments[0].key = Some("08B".into());
    bad.segments[0].confidence = 90;
    assert!(bad.validate().is_err());
    bad = profile.clone();
    bad.algorithm_version += 1;
    assert!(bad.validate().is_err());
    let bytes = serde_json::to_vec(&profile).unwrap();
    assert_eq!(
        serde_json::from_slice::<LocalKeyProfile>(&bytes).unwrap(),
        profile
    );
    fs::write(file.path(), b"replacement").unwrap();
    assert_ne!(
        fingerprint(file.path(), &cancel).unwrap(),
        profile.source_sha256
    );
    cancel.request();
    assert!(fingerprint(file.path(), &cancel).is_err());
    assert!(analyze(file.path(), &cancel).is_err());
}

#[test]
#[ignore = "paired 30 s timed two-deck renders with cancellable local key analysis"]
fn a06_equal_duration_local_key_analysis_with_two_decks() {
    let file = cadence(&MODULATION, true);
    for active in [false, true] {
        eprintln!("A06 complete-file local harmony analysis active={active}");
        let (audio, count) = std::thread::scope(|scope| {
            let (finish, stop) = std::sync::mpsc::channel::<()>();
            let path = file.path();
            let worker = scope.spawn(move || {
                let mut completed = 0;
                while stop.recv_timeout(Duration::from_millis(200))
                    == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    if active {
                        let profile = analyze(path, &Cancellation::default()).unwrap();
                        assert_eq!(profile.segments.len(), 2);
                        completed += 1;
                    }
                }
                completed
            });
            let result = measure_two_decks(30, || {});
            drop(finish);
            (result, worker.join().unwrap())
        });
        eprintln!("A06 complete modulation analyses={count}");
        assert!(if active { count >= 5 } else { count == 0 });
        assert_eq!(
            audio,
            (0, 0, 0, 0),
            "render overruns, deadline misses, signal errors, render allocations"
        );
    }
}

#[test]
#[ignore = "explicit external local harmony corpus, no development examples or annotations are read"]
fn a06_corpus_intervals_tsv() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::var_os("DRDJPRO_HARMONY_CORPUS").ok_or("Set DRDJPRO_HARMONY_CORPUS")?,
    );
    let mut dirs = vec![root.clone()];
    let mut paths = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                dirs.push(entry.path());
            } else if ty.is_file() && crate::audio_formats::supported(&entry.path()) {
                paths.push(entry.path());
            }
        }
    }
    if paths.is_empty() {
        return Err("No supported audio files in the harmony corpus".into());
    }
    paths.sort();
    let destination = std::env::var_os("DRDJPRO_HARMONY_TSV").map(PathBuf::from);
    let mut temporary = destination
        .as_ref()
        .map(|d| {
            tempfile::NamedTempFile::new_in(
                d.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
        })
        .transpose()?;
    let mut out: Box<dyn Write + '_> = match temporary.as_mut() {
        Some(file) => Box::new(io::BufWriter::new(file)),
        None => Box::new(io::stdout().lock()),
    };
    writeln!(out, "path\tstart_ms\tend_ms\tkey\tconfidence")?;
    for path in paths {
        let name = path
            .strip_prefix(&root)?
            .components()
            .map(|c| c.as_os_str().to_str().ok_or("Non-UTF8 corpus path"))
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        let escaped = if name.contains(['\t', '\n', '\r', '"']) {
            format!("\"{}\"", name.replace('"', "\"\""))
        } else {
            name
        };
        let profile = analyze(&path, &Cancellation::default())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        for section in profile.segments {
            writeln!(
                out,
                "{escaped}\t{}\t{}\t{}\t{}",
                section.start_ms,
                section.end_ms,
                section.key.as_deref().unwrap_or("unknown"),
                section.confidence
            )?;
        }
    }
    out.flush()?;
    drop(out);
    if let (Some(file), Some(path)) = (temporary, destination) {
        file.persist_noclobber(path)?;
    }
    Ok(())
}
