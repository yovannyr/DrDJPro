//! Header/tag inspection on library/load workers; no PCM decode or tag writes.
pub(crate) mod diagnosis;
use drdjpro_ui_spike::analysis::keys::CamelotKey;
use std::path::Path;
use symphonia::core::meta::{Limit, MetadataOptions, MetadataRevision, StandardTagKey};

/// DJ-confirmed catalog values, independent of estimated key, loudness and source tags.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MusicLabels {
    confirmed_key: Option<CamelotKey>,
    energy: Option<u8>,
}
impl MusicLabels {
    /// Blank inputs explicitly clear the corresponding label.
    pub fn parse(key: &str, energy: &str) -> Result<Self, String> {
        let key = key.trim().to_ascii_uppercase();
        let energy = energy.trim();
        let energy = if energy.is_empty() {
            None
        } else {
            Some(
                energy
                    .parse()
                    .map_err(|_| "Energy must be a whole number from 1 to 10")?,
            )
        };
        Self::from_stored((!key.is_empty()).then_some(key.as_str()), energy)
    }
    pub(crate) fn from_stored(key: Option<&str>, energy: Option<u32>) -> Result<Self, String> {
        let confirmed_key = key
            .map(|value| {
                CamelotKey::parse(value)
                    .filter(|key| key.to_string() == value)
                    .ok_or("Use a Camelot key from 1A to 12B")
            })
            .transpose()?;
        if energy.is_some_and(|e| !(1..=10).contains(&e)) {
            return Err("Energy must be a whole number from 1 to 10".into());
        }
        Ok(Self {
            confirmed_key,
            energy: energy.map(|e| e as u8),
        })
    }
    pub fn key(self) -> Option<CamelotKey> {
        self.confirmed_key
    }
    pub fn energy(self) -> Option<u8> {
        self.energy
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackMetadata {
    pub music_labels: MusicLabels,
    /// Explicit XML/API metadata wins over later passive file-tag inspection.
    pub prefer_imported_tags: bool,
    pub analysis: Option<crate::audio_analyzer::TrackAnalysisResult>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub duration_exact: bool,
    pub scanned: bool,
    pub error: Option<String>,
}
pub const MAX_TEXT: usize = 512;
pub const MAX_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
impl TrackMetadata {
    /// Passive header refresh must not revoke explicitly imported tag ownership.
    pub(crate) fn retain_imported_tags(&mut self, existing: &Self) {
        if existing.prefer_imported_tags && !self.prefer_imported_tags {
            if !existing.title.is_empty() {
                self.title = existing.title.clone();
            }
            if !existing.artist.is_empty() {
                self.artist = existing.artist.clone();
            }
            if !existing.album.is_empty() {
                self.album = existing.album.clone();
            }
            self.prefer_imported_tags = true;
        }
    }
    /// Retry supported legacy parser failures; successful warnings remain cached.
    pub(crate) fn needs_refresh(&self, path: &Path) -> bool {
        !self.scanned
            || (crate::audio_formats::supported(path)
                && matches!(
                    self.error.as_deref(),
                    Some("malformed stream: id3v2: invalid text encoding")
                        | Some("unsupported feature: id3v2: compressed frames are not supported")
                        | Some("unsupported feature: wav: unsupported wave format")
                        | Some(
                            "unsupported feature: core (probe): no suitable format reader found"
                        )
                ))
    }
    pub fn validate(&self) -> bool {
        self.analysis.as_ref().is_none_or(|a| a.validate())
            && [&self.title, &self.artist, &self.album]
                .iter()
                .all(|s| s.chars().count() <= MAX_TEXT)
            && self
                .error
                .as_ref()
                .is_none_or(|s| s.chars().count() <= MAX_TEXT)
            && self.duration_ms.is_none_or(|v| v <= MAX_DURATION_MS)
            && (!self.duration_exact || self.duration_ms.is_some())
    }
    pub fn duration_text(&self) -> String {
        self.duration_ms.map_or_else(
            || "—".into(),
            |ms| {
                format!(
                    "{}{:02}:{:02}",
                    if self.duration_exact { "" } else { "~" },
                    ms / 60000,
                    (ms / 1000) % 60
                )
            },
        )
    }
}
pub(crate) fn text(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect()
}
fn tags(output: &mut TrackMetadata, revision: &MetadataRevision) {
    for tag in revision.tags() {
        let target = match tag.std_key {
            Some(StandardTagKey::TrackTitle) => &mut output.title,
            Some(StandardTagKey::Artist) => &mut output.artist,
            Some(StandardTagKey::Album) => &mut output.album,
            _ => continue,
        };
        let value = text(&tag.value.to_string());
        if !value.is_empty() {
            *target = value;
        }
    }
}
fn header_options() -> MetadataOptions {
    MetadataOptions {
        limit_metadata_bytes: Limit::Maximum(512 * 1024),
        limit_visual_bytes: Limit::Maximum(0),
    }
}

pub fn read(path: &Path) -> TrackMetadata {
    read_cancellable(path, &crate::cancellation::Cancellation::default())
        .expect("a fresh cancellation token cannot be cancelled")
}

pub(crate) fn read_cancellable(
    path: &Path,
    cancel: &crate::cancellation::Cancellation,
) -> Result<TrackMetadata, String> {
    probe_cancellable(path, cancel).map(|(metadata, _)| metadata)
}

/// Report whether an audio track was found separately from recoverable header warnings.
pub(crate) fn probe_cancellable(
    path: &Path,
    cancel: &crate::cancellation::Cancellation,
) -> Result<(TrackMetadata, bool), String> {
    cancel.check()?;
    if crate::opus_audio::is_opus(path).unwrap_or(false) {
        let result = crate::opus_audio::metadata(path, cancel);
        let readable = result.is_ok();
        let metadata = result.unwrap_or_else(|error| TrackMetadata {
            scanned: true,
            error: Some(text(&error)),
            ..Default::default()
        });
        cancel.check()?;
        return Ok((metadata, readable));
    }
    let result = (|| -> Result<TrackMetadata, Box<dyn std::error::Error>> {
        let (mut probed, warning) = crate::media_input::probe(path, &header_options())?;
        let mut output = TrackMetadata {
            scanned: true,
            error: warning,
            ..Default::default()
        };
        if let Some(metadata) = probed.metadata.get() {
            if let Some(revision) = metadata.current() {
                tags(&mut output, revision);
            }
        }
        if let Some(revision) = probed.format.metadata().current() {
            tags(&mut output, revision);
        }
        let track = probed
            .format
            .default_track()
            .ok_or("No default audio track")?;
        let params = &track.codec_params;
        if let (Some(frames), Some(rate)) = (params.n_frames, params.sample_rate) {
            if rate > 0 {
                output.duration_ms = frames
                    .checked_mul(1000)
                    .map(|v| v / u64::from(rate))
                    .filter(|v| *v <= MAX_DURATION_MS);
            }
        }
        Ok(output)
    })();
    let readable = result.is_ok();
    let metadata = result.unwrap_or_else(|e| TrackMetadata {
        scanned: true,
        error: Some(text(&e.to_string())),
        ..Default::default()
    });
    cancel.check()?;
    Ok((metadata, readable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{tagged_mp3, tagged_wav, wav_file, TempFile};
    #[test]
    fn music_labels_validate_known_unknown_and_invalid_inputs() {
        let known = MusicLabels::parse(" 8b ", "10").unwrap();
        assert_eq!(known.key().unwrap().to_string(), "8B");
        assert_eq!(known.energy(), Some(10));
        for key in ["0A", "13B", "08A", "+1B", "major"] {
            assert!(MusicLabels::parse(key, "").is_err(), "{key}");
        }
        for energy in ["-1", "0", "11", "2.3"] {
            assert!(MusicLabels::parse("", energy).is_err(), "{energy}");
        }
        assert!(MusicLabels::parse(" ", "10").unwrap().key().is_none());
        assert!(MusicLabels::parse("8B", " ").unwrap().energy().is_none());
    }
    #[test]
    fn reads_utf8_mp3_tags_and_header_duration_without_pcm_decode() {
        let file = tagged_mp3("Canción 日本", "DJ Ñ", "Album 2026");
        let meta = read(file.path());
        assert_eq!(meta.title, "Canción 日本");
        assert_eq!(meta.artist, "DJ Ñ");
        assert_eq!(meta.album, "Album 2026");
        assert!(meta.error.is_none());
        assert!(meta.scanned);
        assert!((8900..9300).contains(&meta.duration_ms.unwrap()));
    }
    #[test]
    fn reads_wav_info_tags_and_handles_missing_tags_without_inventing_values() {
        let file = tagged_wav("WAV title", "WAV artist", "WAV album");
        let meta = read(file.path());
        assert_eq!(meta.title, "WAV title");
        assert_eq!(meta.artist, "WAV artist");
        assert_eq!(meta.album, "WAV album");
        assert_eq!(meta.duration_ms, Some(1000));
        assert!(meta.error.is_none());
        let blank = wav_file(1, 480);
        let meta = read(blank.path());
        assert_eq!(meta.title, "");
        assert_eq!(meta.artist, "");
        assert_eq!(meta.album, "");
        assert_eq!(meta.duration_ms, Some(10));
        assert!(meta.error.is_none());
    }
    #[test]
    fn invalid_or_missing_media_has_unknown_duration_and_explicit_error() {
        let file = TempFile::new("mp3", b"not audio");
        let meta = read(file.path());
        assert!(meta.error.is_some());
        assert_eq!(meta.duration_ms, None);
        assert_eq!(meta.duration_text(), "—");
        assert!(read(Path::new("missing-track.wav")).error.is_some());
    }
    #[test]
    fn text_and_duration_bounds_are_enforced_and_control_characters_are_removed() {
        let file = tagged_mp3(&format!("  {}\n", "É".repeat(600)), "", "");
        let meta = read(file.path());
        assert_eq!(meta.title.chars().count(), MAX_TEXT);
        assert!(meta.validate());
        let invalid = TrackMetadata {
            duration_ms: Some(u64::MAX),
            ..Default::default()
        };
        assert!(!invalid.validate());
        assert_eq!(text("  One\nTwo  "), "OneTwo");
    }
}
