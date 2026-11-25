//! Offline arrangement clock, automation and project persistence. Source marks are milliseconds;
//! the rendering clock uses 48,000 stereo frames per second throughout.
mod audio;
mod video;
pub(crate) use audio::{audition, render_wav};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub(crate) use video::render_video;

pub(super) const RATE: u32 = 48_000;
const MAX_MS: u64 = 6 * 60 * 60 * 1000;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Point {
    pub ms: u64,
    pub value: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Clip {
    pub path: PathBuf,
    pub title: String,
    pub start_ms: u64,
    pub source_ms: u64,
    pub length_ms: u64,
    pub source_bpm: Option<f64>,
    pub first_beat_ms: Option<u64>,
    pub gain: Vec<Point>,
    pub eq: Vec<Point>,
    pub fader: Vec<Point>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Project {
    pub version: u8,
    pub bpm: f64,
    pub clips: Vec<Clip>,
}
impl Project {
    pub(crate) fn new() -> Self {
        Self {
            version: 1,
            bpm: 120.,
            clips: vec![],
        }
    }
    pub(crate) fn end_ms(&self) -> u64 {
        self.clips
            .iter()
            .map(|c| c.start_ms.saturating_add(c.length_ms))
            .max()
            .unwrap_or(0)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.clips.len() > 128 || !(20.0..=400.).contains(&self.bpm) {
            return Err("Invalid arrangement version, tempo or clip count.".into());
        }
        let mut edges = Vec::new();
        for (i, c) in self.clips.iter().enumerate() {
            let bad = |msg: &str| format!("Clip {} ({}): {msg}", i + 1, c.title);
            if c.path.as_os_str().is_empty()
                || c.title.is_empty()
                || c.title.len() > 1024
                || c.title.chars().any(char::is_control)
                || c.length_ms == 0
                || c.start_ms.saturating_add(c.length_ms) > MAX_MS
                || c.source_ms > 604800000
                || c.first_beat_ms.is_some_and(|n| n > 604800000)
                || c.source_bpm.is_some_and(|b| !(20.0..=400.).contains(&b))
            {
                return Err(bad("invalid time, title or source tempo"));
            }
            for (points, min, max) in [(&c.gain, -60., 12.), (&c.eq, -24., 24.), (&c.fader, 0., 1.)]
            {
                if points.len() > 1024
                    || points
                        .iter()
                        .any(|p| p.ms > c.length_ms || !(min..=max).contains(&p.value))
                    || points.windows(2).any(|p| p[0].ms >= p[1].ms)
                {
                    return Err(bad("automation points must have increasing times inside the clip and valid values"));
                }
            }
            edges.push((c.start_ms, 1i8));
            edges.push((c.start_ms + c.length_ms, -1i8));
        }
        edges.sort_unstable();
        let mut count = 0;
        for (_, delta) in edges {
            count += delta;
            if count > 8 {
                return Err("At most eight overlapping clips are supported.".into());
            }
        }
        Ok(())
    }
    pub(crate) fn interval(&self, start: u64, end: u64) -> Result<(), String> {
        self.validate()?;
        if start >= end || end > self.end_ms() {
            return Err("Mark a non-empty interval inside the arrangement.".into());
        }
        Ok(())
    }
    fn protected_path(&self, target: &Path) -> Result<(), String> {
        let canonical = target.canonicalize().ok();
        if self.clips.iter().any(|c| {
            c.path == target
                || canonical
                    .as_ref()
                    .is_some_and(|p| c.path.canonicalize().as_ref().ok() == Some(p))
        }) {
            return Err("Choose a destination different from the source recordings.".into());
        }
        Ok(())
    }
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        self.protected_path(path)?;
        let base = path
            .parent()
            .ok_or("No project folder.")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let mut relative = self.clone();
        for clip in &mut relative.clips {
            if let Ok(suffix) = clip.path.strip_prefix(&base) {
                clip.path = suffix.to_owned();
            }
        }
        let mut temp = tempfile::NamedTempFile::new_in(base).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(temp.as_file_mut(), &relative).map_err(|e| e.to_string())?;
        temp.flush()
            .and_then(|()| temp.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        if temp.as_file().metadata().map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
            return Err("Project exceeds 8 MiB.".into());
        }
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn load(
        path: &Path,
        cancel: &crate::cancellation::Cancellation,
    ) -> Result<Self, String> {
        cancel.check()?;
        let mut file = crate::media_input::open_regular(path).map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        let mut block = [0u8; 16384];
        loop {
            cancel.check()?;
            let count = file.read(&mut block).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            if data.len() + count > 8 * 1024 * 1024 {
                return Err("Project exceeds 8 MiB.".into());
            }
            data.extend_from_slice(&block[..count]);
        }
        let mut p: Self = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        cancel.check()?;
        let base = path
            .parent()
            .ok_or("No project directory.")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        p.validate()?;
        for c in &mut p.clips {
            if c.path.is_relative() {
                c.path = base.join(&c.path);
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod project_input_tests {
    use super::*;
    use crate::cancellation::Cancellation;

    #[test]
    fn cancelled_project_load_preserves_the_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.drdjset");
        Project::new().save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let cancel = Cancellation::default();
        cancel.request();
        assert!(Project::load(&path, &cancel)
            .unwrap_err()
            .contains("cancel"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn timeline_declines_a_named_pipe_even_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("project.fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success());
        let link = dir.path().join("session.drdjset");
        std::os::unix::fs::symlink(&pipe, &link).unwrap();
        for path in [pipe, link] {
            assert!(Project::load(&path, &Cancellation::default()).is_err());
        }
    }
}

#[cfg(test)]
mod acceptance;
