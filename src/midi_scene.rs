//! Editable musical time, independent of the sampler banks and deck transport.
pub(crate) mod transpose;
use crate::{
    cancellation::Cancellation,
    midi_file::{Note, Timing},
    sampler::phrase::{Hit, Pattern},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum Role {
    Melody,
    Accompaniment,
    Pad(u8),
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Event {
    pub note: Note,
    pub role: Role,
    pub approved: bool,
    pub uncertain: bool,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) enum SourceKind {
    #[default]
    Original,
    Bass,
    Vocals,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Source {
    pub path: PathBuf,
    pub start_ms: u64,
    pub length_ms: u64,
    pub digest: [u8; 32],
    #[serde(default)]
    pub kind: SourceKind,
}
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Groove {
    pub mask: u8,
    pub intensity: u8,
    pub window_ms: u16,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scene {
    version: u8,
    pub timing: Timing,
    pub events: Vec<Event>,
    pub source: Option<Source>,
    pub key: Option<String>,
    pub tempo_confirmed: bool,
    pub groove: Groove,
}
impl Default for Scene {
    fn default() -> Self {
        Self {
            version: 1,
            timing: Timing {
                ppq: 480,
                bpm: 120.,
                numerator: 4,
                denominator: 4,
                end_tick: 7680,
            },
            events: vec![],
            source: None,
            key: None,
            tempo_confirmed: false,
            groove: Groove::default(),
        }
    }
}
impl Scene {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let t = self.timing;
        if self.version != 1
            || !(24..=9600).contains(&t.ppq)
            || !(40.0..=240.).contains(&t.bpm)
            || !(1..=16).contains(&t.numerator)
            || ![2, 4, 8, 16].contains(&t.denominator)
            || t.end_tick == 0
            || self.seconds() > 60.
            || self.events.len() > 4096
            || self.groove.mask > 15
            || self.groove.intensity > 100
            || self.groove.window_ms > 150
        {
            return Err("Invalid scene timing, size or groove limits.".into());
        }
        if self
            .key
            .as_ref()
            .is_some_and(|k| crate::chord_passage::canonical_chord(k).as_deref() != Some(k))
        {
            return Err("Invalid stored scene key.".into());
        }
        if self.source.as_ref().is_some_and(|s| {
            !s.path.is_absolute()
                || s.length_ms == 0
                || s.length_ms > 60000
                || s.start_ms > 604800000
        }) {
            return Err("Invalid scene source or interval.".into());
        }
        for e in &self.events {
            let n = e.note;
            let channel = match e.role {
                Role::Melody => 0,
                Role::Accompaniment => 1,
                Role::Pad(0..=3) => 9,
                _ => return Err("Sampler pad must be 1–4.".into()),
            };
            if n.duration == 0
                || n.pitch > 127
                || !(1..=127).contains(&n.velocity)
                || n.channel != channel
                || n.start
                    .checked_add(n.duration)
                    .is_none_or(|end| end > t.end_tick)
            {
                return Err("Note falls outside the selected scene or MIDI range.".into());
            }
        }
        Ok(())
    }
    pub(crate) fn seconds(&self) -> f64 {
        self.timing.end_tick as f64 * 60. / (self.timing.bpm * f64::from(self.timing.ppq))
    }
    pub(crate) fn rendered(&self) -> Vec<Event> {
        let mut events = self.events.clone();
        if self.groove.intensity == 0 || self.groove.mask == 0 {
            return events;
        }
        let range = (f64::from(self.groove.window_ms)
            * self.timing.bpm
            * f64::from(self.timing.ppq)
            * f64::from(self.groove.intensity)
            / 6_000_000.)
            .floor() as i64;
        if range == 0 {
            return events;
        }
        let selected = |r: Role| matches!(r,Role::Pad(p) if p<4 && self.groove.mask&(1<<p)!=0);
        let mut remaining = BTreeMap::new();
        for e in &events {
            if selected(e.role) {
                let room = self
                    .timing
                    .end_tick
                    .saturating_sub(e.note.start.saturating_add(e.note.duration));
                remaining
                    .entry(e.note.start)
                    .and_modify(|v: &mut u32| *v = (*v).min(room))
                    .or_insert(room);
            }
        }
        for e in &mut events {
            if !selected(e.role) || e.note.start == 0 {
                continue;
            }
            let at = e.note.start;
            let hash = at.wrapping_mul(0x9e3779b9).rotate_left(13);
            let ms = 1 + (i64::from(hash >> 1) % range);
            let offset = if hash & 1 == 0 {
                ms.min(i64::from(remaining[&at]))
            } else {
                -ms.min(i64::from(at))
            };
            e.note.start = (i64::from(at) + offset) as u32;
        }
        events
    }
    pub(crate) fn groove_pattern(&self) -> Result<(Pattern, u32), String> {
        self.validate()?;
        let t = self.timing;
        let per_bar = u32::from(t.ppq) * 4;
        if t.numerator != 4
            || t.denominator != 4
            || !t.end_tick.is_multiple_of(per_bar)
            || t.bpm.fract() != 0.
        {
            return Err(
                "Sampler groove preview needs whole 4/4 bars and a whole-number BPM.".into(),
            );
        }
        let bars = t.end_tick / per_bar;
        let mut hits = Vec::new();
        for e in self.rendered() {
            if let Role::Pad(pad) = e.role {
                if e.approved {
                    hits.push(Hit {
                        pad,
                        micros: (f64::from(e.note.start) * 60_000_000. / (t.bpm * f64::from(t.ppq)))
                            .round() as u32,
                    });
                }
            }
        }
        hits.sort_by_key(|h| (h.micros, h.pad));
        if hits.is_empty() {
            return Err("Add a sampler event to hear the groove.".into());
        }
        let pattern = Pattern {
            bpm: t.bpm as u16,
            hits,
        };
        pattern.validate(bars)?;
        Ok((pattern, bars))
    }
    pub(crate) fn save(&self, path: &Path, c: &Cancellation) -> Result<(), String> {
        self.validate()?;
        let mut f = tempfile::NamedTempFile::new_in(path.parent().ok_or("No scene folder.")?)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(f.as_file_mut(), self).map_err(|e| e.to_string())?;
        f.as_file().sync_all().map_err(|e| e.to_string())?;
        c.check()?;
        f.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn read(path: &Path, c: &Cancellation) -> Result<Self, String> {
        let file: File = crate::media_input::open_regular(path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("Scene exceeds 1 MiB.".into());
        }
        c.check()?;
        let scene: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        scene.validate()?;
        Ok(scene)
    }
}

#[cfg(test)]
mod acceptance;
