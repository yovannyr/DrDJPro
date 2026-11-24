//! Source-frame coordinates for the Prepare cue editor, including codec gapless trims.
use super::{clip_diagnosis::stream, preview_clip, PreviewClip};
use crate::{cancellation::Cancellation, looping::frame_time};
use std::path::Path;

/// Count decoded frames without retaining audio. The controller calls this on a worker.
pub(crate) fn source_frames(path: &Path, cancel: &Cancellation) -> Result<(u32, u64), String> {
    let mut frames = 0u64;
    let mut rate = 0;
    stream(path, cancel, |channels, native_rate, data| {
        rate = native_rate;
        frames = frames
            .checked_add((data.len() / usize::from(channels)) as u64)
            .ok_or("Audio frame count overflow")?;
        Ok(false)
    })?;
    if frames == 0 {
        return Err("The file has no cueable audio frames".into());
    }
    Ok((rate, frames))
}

/// Decode a maximum of three seconds, starting at the exact chosen source frame.
pub(crate) fn audition(
    path: &Path,
    start_frame: u64,
    cancel: &Cancellation,
) -> Result<PreviewClip, String> {
    let mut cursor = 0u64;
    let mut format = None;
    let mut samples = Vec::new();
    stream(path, cancel, |channels, rate, data| {
        let width = usize::from(channels);
        let next = cursor
            .checked_add((data.len() / width) as u64)
            .ok_or("Audio frame count overflow")?;
        let stop = start_frame.saturating_add(3 * u64::from(rate));
        let first = cursor.max(start_frame);
        let end = next.min(stop);
        if first < end {
            if format.is_none() {
                samples
                    .try_reserve_exact(3 * rate as usize * width)
                    .map_err(|e| e.to_string())?;
                format = Some((channels, rate));
            }
            let begin_sample = (first - cursor) as usize * width;
            let end_sample = (end - cursor) as usize * width;
            samples.extend_from_slice(&data[begin_sample..end_sample]);
        }
        cursor = next;
        Ok(cursor >= stop)
    })?;
    let (channels, rate) = format.ok_or("Cue is outside the source audio")?;
    let index =
        usize::try_from(start_frame).map_err(|_| "Cue exceeds the addressable frame range")?;
    preview_clip::finish(samples, channels, rate, frame_time(index, rate), cancel)
}

#[cfg(test)]
mod qualification {
    use super::*;
    #[test]
    fn d05_audition_starts_on_the_selected_frame_including_last_frame() {
        let mut pcm = vec![0; 96_000];
        let target = 55_440;
        pcm[target] = 8192;
        pcm[95_999] = -4096;
        let file = crate::test_support::wav_samples(&pcm, 48_000, 1);
        let c = Cancellation::default();
        assert_eq!(source_frames(file.path(), &c).unwrap(), (48_000, 96_000));
        let clip = audition(file.path(), target as u64, &c).unwrap();
        assert_eq!(clip.pcm[0], 0.25);
        assert_eq!(clip.start, std::time::Duration::from_millis(1155));
        assert_eq!(&**audition(file.path(), 95_999, &c).unwrap().pcm, &[-0.125]);
        assert!(audition(file.path(), 96_000, &c).is_err());
    }
}
