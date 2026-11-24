//! Bounded preparation of a source excerpt on the library worker.
use crate::cancellation::Cancellation;
use crate::library::import_review::Stamp;
use crate::looping::{frame_at, frame_time};
use rodio::Source;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub(crate) const PREVIEW_DURATION: Duration = Duration::from_secs(8);
const SAMPLE_DURATION: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SampleVersion {
    path: PathBuf,
    start: Duration,
    stamp: Stamp,
    changed: (i64, i64),
}

impl SampleVersion {
    pub(crate) fn read(path: &Path, start: Duration) -> Result<Option<Self>, String> {
        let file = crate::media_input::open_regular(path)
            .map_err(|e| format!("Cannot inspect sample: {e}"))?;
        Ok(Stamp::for_cache(&file)
            .zip(change_time(&file))
            .map(|(stamp, changed)| Self {
                path: path.to_owned(),
                start,
                stamp,
                changed,
            }))
    }
}

#[cfg(unix)]
fn change_time(file: &std::fs::File) -> Option<(i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().ok()?;
    let time = (metadata.ctime(), metadata.ctime_nsec());
    (time != (0, 0)).then_some(time)
}

#[cfg(windows)]
fn change_time(file: &std::fs::File) -> Option<(i64, i64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{FileBasicInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO},
    };
    let mut info = FILE_BASIC_INFO::default();
    // SAFETY: file owns the handle for the call; the output pointer and size describe info.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle() as isize),
            FileBasicInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    }
    .ok()?;
    (info.ChangeTime != 0).then_some((info.ChangeTime, 0))
}

#[derive(Clone)]
pub(crate) struct PreviewClip {
    pub(super) pcm: Arc<Vec<f32>>,
    pub(super) channels: u16,
    pub(super) rate: u32,
    pub(super) start: Duration,
    sample_version: Option<Arc<SampleVersion>>,
}

fn rodio_clip(
    path: &Path,
    start: Duration,
    duration: Duration,
    cancel: &Cancellation,
) -> Result<PreviewClip, String> {
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
    let limit = frame_at(duration, rate) * usize::from(channels);
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

fn opus_clip(
    path: &Path,
    start: Duration,
    duration: Duration,
    cancel: &Cancellation,
) -> Result<PreviewClip, String> {
    const RATE: u32 = 48_000;
    let first = frame_at(start, RATE);
    let limit = frame_at(duration, RATE);
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

pub(super) fn finish(
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
        sample_version: None,
    })
}

impl PreviewClip {
    /// Allocated PCM bytes. Renderer and cache handles share the same backing allocation.
    pub(super) fn pcm_bytes(&self) -> usize {
        self.pcm.capacity() * size_of::<f32>()
    }

    /// Decode at most three seconds of a local sample on the worker.
    pub(crate) fn open_sample(
        path: &Path,
        start: Duration,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        cancel.check()?;
        let before = SampleVersion::read(path, start)?;
        let mut clip = Self::decode(path, start, SAMPLE_DURATION, cancel)?;
        if before != SampleVersion::read(path, start)? {
            return Err("Sample changed while reading. Select it again.".into());
        }
        // Short samples initially reserve the maximum excerpt. Compact the sole owner off the audio thread.
        if let Some(pcm) = Arc::get_mut(&mut clip.pcm) {
            pcm.shrink_to_fit();
        }
        clip.sample_version = before.map(Arc::new);
        cancel.check()?;
        Ok(clip)
    }

    /// Validate file identity and the OS change time as well as size, mtime and excerpt position.
    /// Filesystems without this evidence always decode again. This metadata check is not a content hash.
    pub(crate) fn open_sample_or_cached(
        path: &Path,
        start: Duration,
        cancel: &Cancellation,
        cached: Option<Self>,
    ) -> Result<Self, String> {
        cancel.check()?;
        if let Some(clip) = cached {
            if let Some(version) = &clip.sample_version {
                if SampleVersion::read(path, start)?.as_ref() == Some(version.as_ref()) {
                    cancel.check()?;
                    return Ok(clip);
                }
            }
        }
        Self::open_sample(path, start, cancel)
    }

    /// Load only the selected excerpt. The retained PCM is shared between interpretations.
    pub(crate) fn open(
        path: &Path,
        start: Duration,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        Self::decode(path, start, PREVIEW_DURATION, cancel)
    }

    fn decode(
        path: &Path,
        start: Duration,
        duration: Duration,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        cancel.check()?;
        if !crate::audio_formats::supported(path) {
            return Err("Select a supported audio file".into());
        }
        if crate::opus_audio::is_opus(path)? {
            opus_clip(path, start, duration, cancel)
        } else {
            rodio_clip(path, start, duration, cancel)
        }
    }
}
