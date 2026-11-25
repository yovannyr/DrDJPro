//! Worker-only, content-addressed four-stem cache. Two replaceable slots;
//! each complete file and each in-progress writer is bounded to 128 MiB.
#[cfg(feature = "ml-import")]
use crate::media::stems::StemCache;
use crate::media::stems::{StemMixState, VoiceCategory};
#[cfg(feature = "ml-import")]
use drdj_ml::stem_tracking::Separator;
use flate2::read::ZlibDecoder;
#[cfg(feature = "ml-import")]
use flate2::{write::ZlibEncoder, Compression};
use sha2::{Digest, Sha256};
#[cfg(feature = "ml-import")]
use std::process::{Command, Stdio};
use std::{
    error::Error,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const LIMIT: u64 = 128 << 20;
const MAX_FRAMES: u64 = 44_100 * 3600;
const MAGIC: &[u8; 8] = b"DRDJST02";
const FRAMES_OFFSET: u64 = 40;
const PAYLOAD_OFFSET: u64 = FRAMES_OFFSET + 8 + 32;
#[cfg(feature = "ml-import")]
const STAGING: &str = "spleeter-preparation.partial";

#[cfg(feature = "ml-import")]
fn reserve_writer(cache: &Path) -> Result<(File, tempfile::NamedTempFile)> {
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join("spleeter-preparation.lock"))?;
    lock.try_lock()
        .map_err(|e| format!("could not reserve stem cache: {e}"))?;
    // Only the owner of the OS lock may reclaim this application's staging file.
    // Process termination releases the lock on all supported platforms.
    match std::fs::remove_file(cache.join(STAGING)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let temp = tempfile::Builder::new()
        .prefix(STAGING)
        .rand_bytes(0)
        .tempfile_in(cache)?;
    Ok((lock, temp))
}

#[cfg(feature = "ml-import")]
fn identity(path: &Path, cancel: &impl Fn() -> bool) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    hash_remaining(&mut file, Sha256::new_with_prefix(MAGIC), cancel)
}

fn hash_remaining(
    file: &mut File,
    mut hash: Sha256,
    cancel: &impl Fn() -> bool,
) -> Result<[u8; 32]> {
    let mut chunk = [0; 65536];
    loop {
        if cancel() {
            return Err("cancelled".into());
        }
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(hash.finalize().into())
}

/// Open snapshot of an atomically published cache entry.
pub struct PreparedStems {
    file: File,
    frames: u64,
}
impl PreparedStems {
    /// Read the existing versioned snapshot. This never prepares a replacement or loads inference models.
    pub fn cached(source: &mut File, cancel: impl Fn() -> bool) -> Result<Self> {
        let cache = std::env::var_os("DRDJ_STEM_CACHE")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or("No prepared stem cache configured")?;
        source.seek(SeekFrom::Start(0))?;
        let key = hash_remaining(source, Sha256::new_with_prefix(MAGIC), &cancel)?;
        Self::open(
            &cache.join(format!("spleeter-v1-{}.stems", key[0] % 2)),
            &key,
            &cancel,
        )
    }
    /// Visit exactly the requested stereo interval on the source's 44.1 kHz stem clock.
    pub fn visit_window(
        self,
        category: VoiceCategory,
        start_ms: u64,
        length_ms: u64,
        cancel: impl Fn() -> bool,
        mut consume: impl FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        let offset = match category {
            VoiceCategory::Vocals => 0,
            VoiceCategory::Drums => 4,
            VoiceCategory::Bass => 8,
            VoiceCategory::Melody => 12,
            _ => return Err("Unsupported prepared stem category".into()),
        };
        let mut pcm = Vec::with_capacity(4096 * 2);
        self.visit_frames(start_ms, length_ms, cancel, |data| {
            pcm.clear();
            for frame in data.as_chunks::<16>().0 {
                for ch in [offset, offset + 2] {
                    pcm.push(f32::from(i16::from_le_bytes([frame[ch], frame[ch + 1]])) / 32768.);
                }
            }
            consume(&pcm)
        })
    }
    /// Aggregate prepared vocal and accompanying stem power in 50-ms windows.
    /// Values use full-scale sample powers, per channel. The final bin may be shorter.
    pub fn vocal_evidence(
        self,
        start_ms: u64,
        length_ms: u64,
        cancel: impl Fn() -> bool,
    ) -> Result<Vec<(f64, f64)>> {
        let mut bins = Vec::new();
        let (mut vocal, mut other, mut n) = (0., 0., 0u32);
        self.visit_frames(start_ms, length_ms, cancel, |bytes| {
            for frame in bytes.as_chunks::<16>().0 {
                for (i, sample) in frame.as_chunks::<2>().0.iter().enumerate() {
                    let power =
                        (f64::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.).powi(2);
                    if i < 2 {
                        vocal += power;
                    } else {
                        other += power;
                    }
                }
                n += 1;
                if n == 2205 {
                    bins.push((vocal / (2. * f64::from(n)), other / (2. * f64::from(n))));
                    vocal = 0.;
                    other = 0.;
                    n = 0;
                }
            }
            Ok(())
        })?;
        if n > 0 {
            bins.push((vocal / (2. * f64::from(n)), other / (2. * f64::from(n))));
        }
        Ok(bins)
    }
    fn visit_frames(
        self,
        start_ms: u64,
        length_ms: u64,
        cancel: impl Fn() -> bool,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let start = start_ms.checked_mul(44100).ok_or("Invalid stem start")? / 1000;
        let end = start
            .checked_add(length_ms.checked_mul(44100).ok_or("Invalid stem length")? / 1000)
            .ok_or("Invalid stem interval")?;
        if end <= start || end > self.frames {
            return Err("Stem does not span the requested passage".into());
        }
        let mut reader = ZlibDecoder::new(self.file);
        let mut buf = vec![0u8; 4096 * 16];
        let mut pos = 0u64;
        while pos < end {
            if cancel() {
                return Err("Cancelled".into());
            }
            let count = (end - pos).min(4096) as usize;
            let bytes = &mut buf[..count * 16];
            reader.read_exact(bytes)?;
            let skip = start.saturating_sub(pos).min(count as u64) as usize;
            if skip < count {
                consume(&bytes[skip * 16..])?;
            }
            pos += count as u64;
        }
        Ok(())
    }
    /// Use the explicit offline import settings, also used by the stem mix command.
    #[cfg(feature = "ml-import")]
    pub fn configured(source: &Path, cancel: impl Fn() -> bool) -> Result<Self> {
        let setting = |name| {
            std::env::var_os(name)
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or_else(|| format!("missing absolute {name}"))
        };
        StemCache::prepare_file(
            source,
            &setting("DRDJ_STEM_CACHE")?,
            &setting("DRDJ_STEM_MODELS")?,
            &setting("DRDJ_ORT_LIBRARY")?,
            cancel,
        )
    }

    fn open(path: &Path, key: &[u8; 32], cancel: &impl Fn() -> bool) -> Result<Self> {
        let mut opts = File::options();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = opts.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > LIMIT {
            return Err("oversize stem cache".into());
        }
        let mut header = [0; 48];
        file.read_exact(&mut header)?;
        let frames = u64::from_le_bytes(header[40..].try_into()?);
        if &header[..8] != MAGIC || &header[8..40] != key || !(1..=MAX_FRAMES).contains(&frames) {
            return Err("stale or invalid stem cache".into());
        }
        let mut checksum = [0; 32];
        file.read_exact(&mut checksum)?;
        if hash_remaining(
            &mut file,
            Sha256::new_with_prefix(frames.to_le_bytes()),
            cancel,
        )? != checksum
        {
            return Err("damaged stem payload".into());
        }
        file.seek(SeekFrom::Start(PAYLOAD_OFFSET))?;
        Ok(Self { file, frames })
    }
}

#[cfg(feature = "ml-import")]
impl StemCache {
    /// Prepare from the original file. A hit opens its existing snapshot before
    /// loading any model. Publish only after EOF, decoder success and a final
    /// source identity check. Competing writers never expose partial streams.
    pub fn prepare_file(
        source: &Path,
        cache: &Path,
        models: &Path,
        runtime: &Path,
        cancel: impl Fn() -> bool,
    ) -> Result<PreparedStems> {
        if !cache.is_absolute() {
            return Err("stem cache must be an absolute directory".into());
        }
        let key = identity(source, &cancel)?;
        std::fs::create_dir_all(cache)?;
        let slot = cache.join(format!("spleeter-v1-{}.stems", key[0] % 2));
        if let Ok(hit) = PreparedStems::open(&slot, &key, &cancel) {
            return Ok(hit);
        }
        if cancel() {
            return Err("cancelled".into());
        }
        let (_reservation, mut temp) = reserve_writer(cache)?;
        if let Ok(hit) = PreparedStems::open(&slot, &key, &cancel) {
            return Ok(hit);
        }
        if cancel() {
            return Err("cancelled".into());
        }
        let mut separator = Separator::load(models, runtime)?;
        temp.write_all(MAGIC)?;
        temp.write_all(&key)?;
        temp.write_all(&[0; 40])?;
        let mut encoder = ZlibEncoder::new(temp.as_file_mut(), Compression::fast());
        let mut child = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-xerror",
                "-protocol_whitelist",
                "file,pipe",
                "-threads",
                "1",
                "-i",
            ])
            .arg(source)
            .args([
                "-map", "0:a:0", "-ac", "2", "-ar", "44100", "-f", "f32le", "-",
            ])
            .stdout(Stdio::piped())
            .spawn()?;
        let mut frames = 0u64;
        let result = separator.stream(
            child.stdout.take().expect("piped stdout"),
            |block| {
                frames += block.len() as u64;
                if frames > MAX_FRAMES {
                    return Err("stem input exceeds one hour".into());
                }
                let mut encoded = Vec::with_capacity(8192 * 16);
                for chunk in block.chunks(8192) {
                    encoded.clear();
                    for sample in chunk.iter().flatten() {
                        if !sample.is_finite() {
                            return Err("invalid stem output".into());
                        }
                        encoded
                            .extend_from_slice(&((sample * 32768.).round() as i16).to_le_bytes());
                    }
                    encoder.write_all(&encoded)?;
                    if encoder.total_out() + 65536 > LIMIT {
                        return Err("stem cache slot full".into());
                    }
                }
                Ok(())
            },
            &cancel,
        );
        if result.is_err() {
            let _ = child.kill();
        }
        let status = child.wait()?;
        result?;
        if !status.success() || frames == 0 {
            return Err("incomplete stem decode".into());
        }
        encoder.finish()?;
        if temp.as_file().metadata()?.len() > LIMIT || identity(source, &cancel)? != key {
            return Err("source changed or cache slot full".into());
        }
        temp.seek(SeekFrom::Start(PAYLOAD_OFFSET))?;
        let checksum = hash_remaining(
            temp.as_file_mut(),
            Sha256::new_with_prefix(frames.to_le_bytes()),
            &cancel,
        )?;
        temp.seek(SeekFrom::Start(FRAMES_OFFSET))?;
        temp.write_all(&frames.to_le_bytes())?;
        temp.write_all(&checksum)?;
        temp.flush()?;
        if cancel() {
            return Err("cancelled".into());
        }
        let file = temp.persist(&slot)?;
        Ok(PreparedStems { file, frames })
    }
}

impl StemMixState {
    /// Stream a prepared master mix as stereo 44.1 kHz little-endian f32.
    pub fn render_prepared(&self, stems: PreparedStems, mut output: impl Write) -> Result<()> {
        let gains = [
            VoiceCategory::Vocals,
            VoiceCategory::Drums,
            VoiceCategory::Bass,
            VoiceCategory::Melody,
        ]
        .map(|c| {
            let polarity = if self.phase_invert.get(&c) == Some(&true) {
                -1.
            } else {
                1.
            };
            self.get_gain(c) * self.master_gain * polarity
        });
        let mut reader = ZlibDecoder::new(stems.file);
        let mut remaining = stems.frames;
        let mut buf = vec![0u8; 4096 * 16];
        let mut out = Vec::with_capacity(4096 * 8);
        while remaining > 0 {
            let n = remaining.min(4096) as usize;
            let bytes = &mut buf[..n * 16];
            reader.read_exact(bytes)?;
            out.clear();
            for frame in bytes.as_chunks::<16>().0 {
                for ch in 0..2 {
                    let mut sum = 0.;
                    for (i, gain) in gains.iter().enumerate() {
                        let k = i * 4 + ch * 2;
                        sum +=
                            f32::from(i16::from_le_bytes([frame[k], frame[k + 1]])) * gain / 32768.;
                    }
                    out.extend_from_slice(&sum.to_le_bytes());
                }
            }
            output.write_all(&out)?;
            remaining -= n as u64;
        }
        if reader.read(&mut [0])? != 0 {
            return Err("stem frame count mismatch".into());
        }
        Ok(())
    }
}

/// CLI export with explicit per-stem gains in vocals/drums/bass/other order.
#[cfg(feature = "ml-import")]
pub fn mix_command(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<()> {
    let source = args.next().ok_or("expected source path and four gains")?;
    let mut state = StemMixState::new();
    for category in [
        VoiceCategory::Vocals,
        VoiceCategory::Drums,
        VoiceCategory::Bass,
        VoiceCategory::Melody,
    ] {
        let gain: f32 = args
            .next()
            .and_then(|s| s.into_string().ok())
            .ok_or("expected four gains")?
            .parse()?;
        if !(0.0..=1.0).contains(&gain) {
            return Err("each gain must be 0 through 1".into());
        }
        state.set_gain(category, gain);
    }
    if args.next().is_some() {
        return Err("unexpected stem mix argument".into());
    }
    let stems = PreparedStems::configured(Path::new(&source), || false)?;
    state.render_prepared(stems, std::io::stdout().lock())
}

#[cfg(all(test, feature = "ml-import"))]
mod tests {
    use super::*;

    #[test]
    fn concurrent_preparations_share_one_recoverable_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STAGING), b"abandoned writer").unwrap();
        let (lock, mut staging) = reserve_writer(dir.path()).unwrap();
        assert_eq!(staging.path(), dir.path().join(STAGING));
        assert_eq!(staging.as_file().metadata().unwrap().len(), 0);
        staging.write_all(b"active writer").unwrap();
        assert!(reserve_writer(dir.path()).is_err());
        assert_eq!(std::fs::read(staging.path()).unwrap(), b"active writer");
        drop(staging);
        drop(lock);
        assert!(reserve_writer(dir.path()).is_ok());
    }

    #[test]
    fn cached_stems_mix_by_category_without_loading_models() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("source");
        std::fs::write(&song, b"source identity").unwrap();
        let key = identity(&song, &|| false).unwrap();
        let slot = dir.path().join(format!("spleeter-v1-{}.stems", key[0] % 2));
        let mut f = File::create(&slot).unwrap();
        f.write_all(MAGIC).unwrap();
        f.write_all(&key).unwrap();
        f.write_all(&1u64.to_le_bytes()).unwrap();
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        for sample in [4096i16, -4096, 2048, -2048, 1024, -1024, 512, -512] {
            z.write_all(&sample.to_le_bytes()).unwrap();
        }
        let compressed = z.finish().unwrap();
        let mut hash = Sha256::new_with_prefix(1u64.to_le_bytes());
        hash.update(&compressed);
        f.write_all(&hash.finalize()).unwrap();
        f.write_all(&compressed).unwrap();
        drop(f);
        let hit =
            StemCache::prepare_file(&song, dir.path(), dir.path(), dir.path(), || false).unwrap();
        let mut mix = StemMixState::new();
        mix.set_gain(VoiceCategory::Drums, 0.);
        mix.toggle_phase(VoiceCategory::Vocals);
        let mut output = Vec::new();
        mix.render_prepared(hit, &mut output).unwrap();
        let value = f32::from_le_bytes(output[..4].try_into().unwrap());
        assert_eq!(value, (-4096. + 1024. + 512.) / 32768.);
        assert_eq!(output.len(), 8);
        let mut damaged = std::fs::OpenOptions::new().write(true).open(&slot).unwrap();
        damaged.seek(SeekFrom::Start(PAYLOAD_OFFSET)).unwrap();
        damaged.write_all(&[0xff]).unwrap();
        drop(damaged);
        assert!(PreparedStems::open(&slot, &key, &|| false).is_err());
        assert!(
            StemCache::prepare_file(&song, dir.path(), dir.path(), dir.path(), || true).is_err()
        );
        std::fs::write(&song, b"changed").unwrap();
        assert!(
            StemCache::prepare_file(&song, dir.path(), dir.path(), dir.path(), || false).is_err()
        );
    }
}
