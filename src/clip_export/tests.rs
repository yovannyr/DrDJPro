use super::*;
use crate::{audio::analyze_file, playback::clip_diagnosis::scan, test_support::wav_samples};
use std::{fs, io::SeekFrom};

#[test]
fn exports_the_whole_corrected_track_and_preserves_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("song.wav");
    let rate = 48_000u32;
    let samples: Vec<i16> = (0..rate * 9)
        .map(|i| {
            let phase = std::f64::consts::TAU * f64::from(i % 48) / 48.0;
            let amplitude = if (rate * 3..rate * 6).contains(&i) {
                1.8
            } else {
                0.6
            };
            ((phase.sin() * amplitude).clamp(-1.0, 1.0) * 32767.0).round() as i16
        })
        .collect();
    let fixture = wav_samples(&samples, rate, 1);
    fs::copy(fixture.path(), &source).unwrap();
    let original = fs::read(&source).unwrap();
    let cancel = Cancellation::default();
    let report = scan(&source, &cancel).unwrap();
    assert!(!report.markers.is_empty());
    let target = export(&source, &report, dir.path(), &cancel).unwrap();
    assert_eq!(target, dir.path().join("song-clip-preview.wav"));
    let result = analyze_file(&target).unwrap();
    assert_eq!(
        (result.channels, result.sample_rate, result.frames),
        (1, rate, samples.len() as u64)
    );
    let copy = fs::read(&target).unwrap();
    let second = rate as usize * 2;
    let pcm = &copy[44..];
    let input = &original[44..];
    assert_eq!(&pcm[..second], &input[..second]);
    assert_ne!(&pcm[4 * second..5 * second], &input[4 * second..5 * second]);
    assert_eq!(&pcm[8 * second..], &input[8 * second..]);
    assert!(export(&source, &report, dir.path(), &cancel).is_err());
    assert_eq!(fs::read(&target).unwrap(), copy);
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn never_overwrites_a_target_created_during_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("song-clip-preview.wav");
    let result = publish(&target, &Cancellation::default(), |file| {
        file.write_all(b"work in progress").map_err(write_error)?;
        fs::write(&target, b"other export").map_err(write_error)
    });
    assert!(result.is_err());
    assert_eq!(fs::read(target).unwrap(), b"other export");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn cancellation_after_writing_discards_the_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("cancelled.wav");
    let cancel = Cancellation::default();
    let result = publish(&target, &cancel, |file| {
        file.write_all(b"first frames").map_err(write_error)?;
        cancel.request();
        Ok(())
    });
    assert_eq!(result.unwrap_err(), "Operation cancelled");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// Stops writes after the header and the first few frames, like a full destination volume.
struct LimitedFile<'a> {
    file: &'a mut File,
    remaining: usize,
}

impl Write for LimitedFile<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("test volume full"));
        }
        let len = bytes.len().min(self.remaining);
        let written = self.file.write(&bytes[..len])?;
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for LimitedFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

#[test]
fn midstream_write_failure_discards_all_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("partial.wav");
    let cancel = Cancellation::default();
    let result = publish(&target, &cancel, |file| {
        let mut limited = LimitedFile {
            file,
            remaining: 64,
        };
        write_wav(&mut limited, &cancel, |consume| {
            consume(2, 48_000, &[0.125; 128])
        })
    });
    assert!(result.unwrap_err().contains("test volume full"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn rejects_invalid_samples_changed_formats_and_oversized_riff_lengths() {
    let cancel = Cancellation::default();
    for (ch, rate, pcm) in [
        (0, 48_000, &[0.0][..]),
        (2, 48_000, &[0.0][..]),
        (1, 0, &[0.0][..]),
        (1, 48_000, &[f32::NAN][..]),
        (1, 48_000, &[f32::INFINITY][..]),
        (1, 48_000, &[][..]),
    ] {
        let mut out = io::Cursor::new(Vec::new());
        assert!(write_wav(&mut out, &cancel, |sink| sink(ch, rate, pcm)).is_err());
    }
    let mut out = io::Cursor::new(Vec::new());
    assert!(write_wav(&mut out, &cancel, |sink| {
        sink(1, 48_000, &[0.25])?;
        sink(2, 48_000, &[0.5, 0.75])
    })
    .is_err());
    let largest_even = u32::MAX - 37;
    assert_eq!(new_data_size(largest_even - 2, 1).unwrap(), largest_even);
    assert!(new_data_size(largest_even, 1).is_err());
    assert!(new_data_size(0, usize::MAX).is_err());
}
