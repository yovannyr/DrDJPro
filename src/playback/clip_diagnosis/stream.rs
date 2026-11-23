use crate::cancellation::Cancellation;
use std::{io::ErrorKind, path::Path};
use symphonia::core::{audio::SampleBuffer, errors::Error};

/// Decode native frames without seeking, resampling or merging corrupt/missing packets.
pub(super) fn stream(
    path: &Path,
    cancel: &Cancellation,
    mut consume: impl FnMut(u16, u32, &[f32]) -> Result<bool, String>,
) -> Result<(), String> {
    let mut format_seen = None;
    let mut checked = |channels: u16, rate: u32, pcm: &[f32]| {
        cancel.check()?;
        if !(1..=2).contains(&channels) || !(8000..=192000).contains(&rate) {
            return Err("Clip diagnosis supports mono/stereo at 8–192 kHz".into());
        }
        if *format_seen.get_or_insert((channels, rate)) != (channels, rate) {
            return Err("Audio format changed during track".into());
        }
        if !pcm.len().is_multiple_of(usize::from(channels)) || pcm.iter().any(|v| !v.is_finite()) {
            return Err("Invalid decoded audio block".into());
        }
        consume(channels, rate, pcm)
    };
    if crate::opus_audio::is_opus(path)? {
        return crate::opus_audio::decode_stream_until(path, cancel, checked);
    }
    let options = symphonia::core::formats::FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let (probed, _) = crate::media_input::probe_with_format(path, &Default::default(), &options)
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("No audio track")?;
    let track_id = track.id;
    let metadata_rate = track.codec_params.sample_rate;
    let window = if track.codec_params.codec == symphonia::core::codecs::CODEC_TYPE_AAC {
        super::m4a::presentation(path, track_id, &track.codec_params, cancel)?
    } else {
        None
    };
    let mut decoded_frames = 0u64;
    let mut source_spec = None;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .map_err(|e| e.to_string())?;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        cancel.check()?;
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio = decoder.decode(&packet).map_err(|e| e.to_string())?;
        let spec = *audio.spec();
        if window.is_some() && Some(spec.rate) != metadata_rate {
            return Err("Decoded AAC sample rate differs from its edit timeline".into());
        }
        if *source_spec.get_or_insert(spec) != spec {
            return Err("Audio format changed during track".into());
        }
        let width = spec.channels.count();
        // Limit conversion scratch to one codec block. FLAC permits 65535 frames per block.
        if !(1..=2).contains(&width) || audio.capacity() > 65536 {
            return Err("Unsupported audio block size or channel layout".into());
        }
        if buffer
            .as_ref()
            .is_none_or(|b| b.capacity() < audio.capacity() * width)
        {
            buffer = Some(SampleBuffer::new(audio.capacity() as u64, spec));
        }
        let samples = buffer.as_mut().unwrap();
        samples.copy_interleaved_ref(audio);
        let pcm = samples.samples();
        let next = decoded_frames
            .checked_add((pcm.len() / width) as u64)
            .ok_or("Track is too long")?;
        let from = window
            .as_ref()
            .map_or(decoded_frames, |w| decoded_frames.max(w.start))
            .min(next);
        let end = window.as_ref().map_or(next, |w| next.min(w.end)).max(from);
        let first = (from - decoded_frames) as usize * width;
        let last = (end - decoded_frames) as usize * width;
        decoded_frames = next;
        for block in pcm[first..last].chunks(4096 * width) {
            if checked(width as u16, spec.rate, block)? {
                return Ok(());
            }
        }
    }
    if window.is_some_and(|w| decoded_frames < w.end) {
        return Err("AAC ends before the selected presentation".into());
    }
    cancel.check()
}
