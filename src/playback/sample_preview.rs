//! Bounded in-memory cache of decoded sample excerpts. Only the control plane owns this queue.
use super::{PreviewClip, MAX_SAMPLE_PREVIEW_BYTES};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

// Bound row/path overhead as well, even for very short files.
const MAX_ENTRIES: usize = 128;

#[derive(Default)]
pub(crate) struct SamplePreviewCache {
    entries: VecDeque<(PathBuf, PreviewClip)>,
    bytes: usize,
}

impl SamplePreviewCache {
    /// A RAM-only candidate. The decoder worker checks its on-disk version before publication.
    pub(crate) fn get(&mut self, path: &Path) -> Option<PreviewClip> {
        let index = self.entries.iter().position(|(p, _)| p == path)?;
        let entry = self.entries.remove(index)?;
        let clip = entry.1.clone();
        self.entries.push_back(entry);
        Some(clip)
    }

    pub(crate) fn insert(&mut self, path: PathBuf, clip: PreviewClip) {
        if let Some(index) = self.entries.iter().position(|(p, _)| p == &path) {
            if let Some((_, old)) = self.entries.remove(index) {
                self.bytes -= old.pcm_bytes();
            }
        }
        let cost = clip.pcm_bytes();
        if cost > MAX_SAMPLE_PREVIEW_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || cost > MAX_SAMPLE_PREVIEW_BYTES - self.bytes {
            let Some((_, old)) = self.entries.pop_front() else {
                return;
            };
            self.bytes -= old.pcm_bytes();
        }
        self.entries.push_back((path, clip));
        self.bytes += cost;
    }

    /// Combined allocated capacities of the PCM vectors resident in the cache.
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}
