//! Worker-only local key estimation. Source-time sections have no dependency on beatgrid or tags.
mod sections;
mod spectrum;
pub(crate) use spectrum::peak_chroma;
#[cfg(test)]
mod tests;

use crate::{cancellation::Cancellation, library::import_review::Stamp, playback::clip_diagnosis};
use drdjpro_ui_spike::analysis::keys::CamelotKey;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const ALGORITHM_VERSION: u16 = 1;
const MAX_SECTIONS: usize = 4096;
const MAX_HALF_SECONDS: usize = 24 * 60 * 60 * 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeySection {
    pub start_ms: u64,
    pub end_ms: u64,
    pub key: Option<String>,
    /// Strength of local tonal evidence on a 0–100 scale.
    pub confidence: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalKeyProfile {
    pub algorithm_version: u16,
    pub source_sha256: [u8; 32],
    pub duration_ms: u64,
    pub segments: Vec<KeySection>,
}
impl LocalKeyProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.algorithm_version != ALGORITHM_VERSION {
            return Err("Unsupported local key analysis version".into());
        }
        if self.duration_ms == 0
            || self.duration_ms > MAX_HALF_SECONDS as u64 * 500
            || self.segments.is_empty()
            || self.segments.len() > MAX_SECTIONS
        {
            return Err("Invalid local key duration or section count".into());
        }
        let mut end = 0;
        for section in &self.segments {
            if section.start_ms != end || section.end_ms <= end || section.end_ms > self.duration_ms
            {
                return Err("Invalid local key section bounds".into());
            }
            match section.key.as_deref() {
                Some(raw)
                    if (1..=100).contains(&section.confidence)
                        && CamelotKey::parse(raw).is_some_and(|key| key.to_string() == raw) => {}
                None if section.confidence == 0 => {}
                _ => return Err("Invalid local key or confidence".into()),
            }
            end = section.end_ms;
        }
        if end != self.duration_ms {
            return Err("Incomplete local key profile".into());
        }
        Ok(())
    }
}

pub(crate) fn fingerprint(path: &Path, cancel: &Cancellation) -> Result<[u8; 32], String> {
    cancel.check()?;
    clip_diagnosis::digest(path, cancel)
}

pub(crate) fn analyze(path: &Path, cancel: &Cancellation) -> Result<LocalKeyProfile, String> {
    cancel.check()?;
    let stamp = Stamp::at(path).ok();
    let source_sha256 = fingerprint(path, cancel)?;
    let mut analyzer = None;
    clip_diagnosis::stream(path, cancel, |channels, rate, pcm| {
        let worker = analyzer.get_or_insert_with(|| spectrum::Spectrum::new(rate));
        worker.push(channels, pcm, cancel)?;
        Ok(false)
    })?;
    let (observations, duration_ms) = analyzer
        .ok_or("No audio frames for local key analysis")?
        .finish()?;
    let segments = sections::summarize(&observations, duration_ms, cancel)?;
    if source_sha256 != fingerprint(path, cancel)? || stamp != Stamp::at(path).ok() {
        return Err("Source changed during local key analysis".into());
    }
    let profile = LocalKeyProfile {
        algorithm_version: ALGORITHM_VERSION,
        source_sha256,
        duration_ms,
        segments,
    };
    profile.validate()?;
    Ok(profile)
}
