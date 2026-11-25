//! Offline, full-track WAV export for a diagnosed source.
use crate::{
    cancellation::Cancellation,
    playback::clip_diagnosis::{stream_processed, ClipReport},
};
use std::{
    fs::File,
    io::{self, Seek, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const ALREADY_EXISTS: &str = "A clip preview with this name already exists";

/// Write a separate processed copy at the source sample rate. Only a completed export
/// becomes visible under the requested name, even when several exports run concurrently.
pub(crate) fn export(
    path: &Path,
    report: &ClipReport,
    destination_dir: &Path,
    cancel: &Cancellation,
) -> Result<PathBuf, String> {
    let mut name = path.file_stem().ok_or("Track has no filename")?.to_owned();
    name.push("-clip-preview.wav");
    let target = destination_dir.join(name);
    publish(&target, cancel, |file| {
        write_wav(file, cancel, |consume| {
            stream_processed(path, report, cancel, consume)
        })
    })?;
    Ok(target)
}

fn publish(
    target: &Path,
    cancel: &Cancellation,
    render: impl FnOnce(&mut File) -> Result<(), String>,
) -> Result<(), String> {
    cancel.check()?;
    match std::fs::symlink_metadata(target) {
        Ok(_) => return Err(ALREADY_EXISTS.into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(write_error(e)),
    }
    let dir = target
        .parent()
        .ok_or("Export path has no parent directory")?;
    let mut temp = NamedTempFile::new_in(dir).map_err(write_error)?;
    render(temp.as_file_mut())?;
    cancel.check()?;
    temp.as_file().sync_all().map_err(write_error)?;
    cancel.check()?;
    temp.persist_noclobber(target)
        .map_err(|error| match error.error.kind() {
            io::ErrorKind::AlreadyExists => ALREADY_EXISTS.to_owned(),
            _ => write_error(error.error),
        })?;
    Ok(())
}

fn write_error(error: io::Error) -> String {
    format!("Cannot write clip preview: {error}")
}

fn new_data_size(previous: u32, samples: usize) -> Result<u32, String> {
    u32::try_from(samples)
        .ok()
        .and_then(|n| n.checked_mul(2))
        .and_then(|bytes| previous.checked_add(bytes))
        .filter(|bytes| *bytes <= u32::MAX - 36)
        .ok_or_else(|| "Track exceeds the size limit for RIFF/WAV".into())
}

type SampleSink<'a> = dyn FnMut(u16, u32, &[f32]) -> Result<(), String> + 'a;

fn write_wav(
    out: &mut (impl Write + Seek),
    cancel: &Cancellation,
    stream: impl FnOnce(&mut SampleSink<'_>) -> Result<(), String>,
) -> Result<(), String> {
    cancel.check()?;
    out.write_all(&[0; 44]).map_err(write_error)?;
    let mut format = None;
    let mut data_size = 0;
    let mut bytes = [0u8; 8192];
    stream(&mut |channels, rate, pcm| {
        if !(1..=2).contains(&channels) || !(1..=192_000).contains(&rate) {
            return Err("Invalid channel count or sample rate for clip preview".into());
        }
        if !pcm.len().is_multiple_of(usize::from(channels)) {
            return Err("Incomplete audio frame in clip preview".into());
        }
        if *format.get_or_insert((channels, rate)) != (channels, rate) {
            return Err("Track format changed during clip export".into());
        }
        data_size = new_data_size(data_size, pcm.len())?;
        for block in pcm.chunks(bytes.len() / 2) {
            cancel.check()?;
            let (words, _) = bytes.as_chunks_mut::<2>();
            for (&sample, dst) in block.iter().zip(words) {
                if !sample.is_finite() {
                    return Err("Non-finite audio sample in clip preview".into());
                }
                // The saturating float-to-int cast also maps the positive rail to i16::MAX.
                let value = (sample * 32768.0).round() as i16;
                dst.copy_from_slice(&value.to_le_bytes());
            }
            out.write_all(&bytes[..2 * block.len()])
                .map_err(write_error)?;
        }
        Ok(())
    })?;
    cancel.check()?;
    let (channels, rate) = format
        .filter(|_| data_size > 0)
        .ok_or("No audio frames to export")?;
    out.rewind().map_err(write_error)?;
    wav_header(out, channels, rate, data_size).map_err(write_error)?;
    out.flush().map_err(write_error)
}

fn wav_header(out: &mut impl Write, channels: u16, rate: u32, size: u32) -> io::Result<()> {
    let alignment = channels * 2;
    let mut header = [0; 44];
    header[..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + size).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // Linear PCM.
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&(rate * u32::from(alignment)).to_le_bytes());
    header[32..34].copy_from_slice(&alignment.to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&size.to_le_bytes());
    out.write_all(&header)
}

#[cfg(test)]
mod tests;
