//! Bounded preparation of a source excerpt on the library worker.
use crate::cancellation::Cancellation;
use crate::looping::{frame_at, frame_time};
use rodio::Source;
use std::{path::Path, sync::Arc, time::Duration};

pub(crate) const PREVIEW_DURATION: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub(crate) struct PreviewClip {
    pub(super) pcm: Arc<Vec<f32>>,
    pub(super) channels: u16,
    pub(super) rate: u32,
    pub(super) start: Duration,
}

fn rodio_clip(path: &Path, start: Duration, cancel: &Cancellation) -> Result<PreviewClip, String> {
    let mut decoder = super::open_pcm_decoder(path)?;
    let channels = decoder.channels();
    let rate = decoder.sample_rate();
    if !(1..=2).contains(&channels) || !(1..=192_000).contains(&rate) {
        return Err("Preview supports mono/stereo sources up to 192 kHz".into());
    }
    if decoder.total_duration().is_some_and(|end| start >= end) {
        return Err("Preview starts after the end of this track".into());
    }
    let frame = frame_at(start, rate);
    let start = frame_time(frame, rate);
    if !start.is_zero() && decoder.try_seek(start).is_err() {
        // Some supported codecs cannot seek precisely. Skip on the worker without retaining PCM.
        decoder = super::open_pcm_decoder(path)?;
        for sample in 0..frame.saturating_mul(usize::from(channels)) {
            if sample.is_multiple_of(4096) {
                cancel.check()?;
            }
            decoder
                .next()
                .ok_or("Preview starts after the end of this track")?;
        }
    }
    cancel.check()?;
    let limit = frame_at(PREVIEW_DURATION, rate) * usize::from(channels);
    let mut pcm = Vec::with_capacity(limit);
    for (index, sample) in decoder.take(limit).enumerate() {
        if index.is_multiple_of(4096) {
            cancel.check()?;
        }
        if !sample.is_finite() {
            return Err("Non-finite audio sample".into());
        }
        pcm.push(sample);
    }
    finish(pcm, channels, rate, start, cancel)
}

fn opus_clip(path: &Path, start: Duration, cancel: &Cancellation) -> Result<PreviewClip, String> {
    const RATE: u32 = 48_000;
    let first = frame_at(start, RATE);
    let limit = frame_at(PREVIEW_DURATION, RATE);
    let mut skipped = 0;
    let mut channels = 1;
    let mut pcm = Vec::new();
    crate::opus_audio::decode_stream_until(path, cancel, |ch, _, samples| {
        channels = ch;
        let width = usize::from(ch);
        let skip = (first - skipped).min(samples.len() / width);
        skipped += skip;
        let samples = &samples[skip * width..];
        if !samples.is_empty() {
            if pcm.is_empty() {
                pcm.reserve_exact(limit * width);
            }
            let count = samples.len().min(limit * width - pcm.len());
            pcm.extend_from_slice(&samples[..count]);
        }
        Ok(pcm.len() == limit * width)
    })?;
    finish(pcm, channels, RATE, frame_time(first, RATE), cancel)
}

fn finish(
    pcm: Vec<f32>,
    channels: u16,
    rate: u32,
    start: Duration,
    cancel: &Cancellation,
) -> Result<PreviewClip, String> {
    cancel.check()?;
    if pcm.is_empty() || !pcm.len().is_multiple_of(usize::from(channels)) {
        return Err("Preview has no complete audio excerpt at that position".into());
    }
    Ok(PreviewClip {
        pcm: Arc::new(pcm),
        channels,
        rate,
        start,
    })
}

impl PreviewClip {
    /// Load only the selected excerpt. The retained PCM is shared between interpretations.
    pub(crate) fn open(
        path: &Path,
        start: Duration,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        cancel.check()?;
        if !crate::audio_formats::supported(path) {
            return Err("Select a supported audio file".into());
        }
        if crate::opus_audio::is_opus(path)? {
            opus_clip(path, start, cancel)
        } else {
            rodio_clip(path, start, cancel)
        }
    }
}
