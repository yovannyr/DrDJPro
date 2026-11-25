//! Worker-side heuristics for repeated flat-topped peaks and a level-matched PFL comparison.
mod detection;
mod m4a;
mod processing;
mod stream;

use super::{preview_clip::SampleVersion, PreviewClip, PREVIEW_DURATION};
use crate::{
    cancellation::Cancellation,
    looping::{frame_at, frame_time},
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
pub(crate) use stream::stream;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClipRegion {
    pub start: Duration,
    pub end: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct ClipReport {
    pub duration: Duration,
    pub markers: Vec<ClipRegion>,
    version: Arc<Version>,
    channels: u16,
    rate: u32,
    frames: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Version {
    Native(SampleVersion),
    Content(PathBuf, [u8; 32]),
}

fn version(path: &Path, cancel: &Cancellation) -> Result<Version, String> {
    if let Some(stamp) = SampleVersion::read(path, Duration::ZERO)? {
        return Ok(Version::Native(stamp));
    }
    Ok(Version::Content(path.to_owned(), digest(path, cancel)?))
}

pub(crate) fn digest(path: &Path, cancel: &Cancellation) -> Result<[u8; 32], String> {
    use sha2::{Digest, Sha256};
    let mut file = crate::media_input::open_regular(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut remaining = len;
    let mut hash = Sha256::new();
    let mut block = [0u8; 32768];
    while remaining > 0 {
        cancel.check()?;
        let n = remaining.min(block.len() as u64) as usize;
        file.read_exact(&mut block[..n])
            .map_err(|e| e.to_string())?;
        hash.update(&block[..n]);
        remaining -= n as u64;
    }
    if file.metadata().map_err(|e| e.to_string())?.len() != len {
        return Err("Source changed while hashing".into());
    }
    cancel.check()?;
    Ok(hash.finalize().into())
}

impl ClipReport {
    pub(crate) fn verify(&self, path: &Path, cancel: &Cancellation) -> Result<(), String> {
        cancel.check()?;
        if *self.version != version(path, cancel)? {
            return Err("Source file changed. Scan it again.".into());
        }
        Ok(())
    }
}

/// Scan complete PCM in frame-aligned blocks while retaining only the resulting time ranges.
pub(crate) fn scan(path: &Path, cancel: &Cancellation) -> Result<ClipReport, String> {
    cancel.check()?;
    let stamp = version(path, cancel)?;
    let mut scan = None;
    stream(path, cancel, |channels, rate, pcm| {
        scan.get_or_insert_with(|| detection::Detector::new(channels, rate))
            .push(pcm)?;
        Ok(false)
    })?;
    let (channels, rate, frames, markers) =
        scan.ok_or("No audio frames for clip diagnosis")?.finish()?;
    cancel.check()?;
    if stamp != version(path, cancel)? {
        return Err("Source file changed during scan".into());
    }
    Ok(ClipReport {
        duration: frame_time(frames, rate),
        markers,
        version: Arc::new(stamp),
        channels,
        rate,
        frames,
    })
}

/// Render the original and processed versions of the same at-most-eight-second source window.
pub(crate) fn comparison(
    path: &Path,
    report: &ClipReport,
    index: usize,
    cancel: &Cancellation,
) -> Result<(PreviewClip, PreviewClip), String> {
    cancel.check()?;
    report.verify(path, cancel)?;
    let marker = report
        .markers
        .get(index)
        .ok_or("Select a clipping marker")?;
    let center = frame_at(marker.start + (marker.end - marker.start) / 2, report.rate);
    let count = frame_at(PREVIEW_DURATION, report.rate).min(report.frames);
    let start = center.saturating_sub(count / 2).min(report.frames - count);
    // Supply the finite filter history even when the wanted window begins inside a marked region.
    let earlier = start.saturating_sub(processing::tap_count(report.rate));
    let preroll = start - earlier;
    let channels = usize::from(report.channels);
    let skip = preroll * channels;
    let end = start + count;
    let mut seen = 0usize;
    let mut raw = Vec::with_capacity((count + preroll) * channels);
    // Use the exact same decoded frame coordinates as the scan, including codec trim.
    stream(path, cancel, |ch, rate, pcm| {
        if (ch, rate) != (report.channels, report.rate) {
            return Err("Source format changed".into());
        }
        let next = seen
            .checked_add(pcm.len() / channels)
            .ok_or("Track is too long")?;
        let from = seen.max(earlier);
        let to = next.min(end);
        if from < to {
            raw.extend_from_slice(&pcm[(from - seen) * channels..(to - seen) * channels]);
        }
        seen = next;
        Ok(seen >= end)
    })?;
    if raw.len() != (preroll + count) * channels {
        return Err("Selected passage is incomplete. Scan the file again.".into());
    }
    let mut filter = processing::Smoother::new(report, earlier);
    let mut processed = raw.clone();
    for block in processed.chunks_mut(4096 * channels) {
        cancel.check()?;
        filter.apply(block);
    }
    processed.drain(..skip);
    raw.drain(..skip);
    processing::match_loudness(
        &mut raw,
        &mut processed,
        report.channels,
        report.rate,
        cancel,
    )?;
    let finish = |pcm| {
        super::preview_clip::finish(
            pcm,
            report.channels,
            report.rate,
            frame_time(start, report.rate),
            cancel,
        )
    };
    cancel.check()?;
    report.verify(path, cancel)?;
    Ok((finish(raw)?, finish(processed)?))
}

/// Export the entire source through the same local peak treatment as the PFL comparison.
/// The A/B loudness adjustment belongs to monitoring only.
pub(crate) fn stream_processed(
    path: &Path,
    report: &ClipReport,
    cancel: &Cancellation,
    mut consume: impl FnMut(u16, u32, &[f32]) -> Result<(), String>,
) -> Result<(), String> {
    cancel.check()?;
    report.verify(path, cancel)?;
    let mut processor = processing::Smoother::new(report, 0);
    let mut block = Vec::new();
    stream(path, cancel, |channels, rate, pcm| {
        if channels != report.channels || rate != report.rate {
            return Err("Source format changed during export".into());
        }
        block.clear();
        block.extend_from_slice(pcm);
        processor.apply(&mut block);
        consume(channels, rate, &block)?;
        Ok(false)
    })?;
    if processor.frame != report.frames {
        return Err("Source length changed during export".into());
    }
    cancel.check()?;
    report.verify(path, cancel)
}

#[cfg(test)]
#[test]
fn a01_content_digest_fallback_detects_a_same_length_change() {
    let a = vec![17; 70_000];
    let file = crate::test_support::TempFile::new("wav", &a);
    let cancel = Cancellation::default();
    let expected = digest(file.path(), &cancel).unwrap();
    let mut b = a;
    b[65_537] = 18;
    std::fs::write(file.path(), b).unwrap();
    assert_ne!(digest(file.path(), &cancel).unwrap(), expected);
    cancel.request();
    assert!(digest(file.path(), &cancel).is_err());
}
