//! Diatonic scale-degree mapping with explicit exceptions for chromatic and out-of-range notes.
use super::{Event, Role, Scene};
use std::collections::BTreeMap;

const ROOTS: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];
const MAJOR: [i16; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i16; 7] = [0, 2, 3, 5, 7, 8, 10];

#[derive(Clone, Copy, Default)]
pub(crate) struct Scale {
    pub root: u8,
    pub minor: bool,
}
impl Scale {
    pub fn new(root: i32, minor: bool) -> Result<Self, String> {
        if !(0..12).contains(&root) {
            return Err("Choose a tonic from C through B.".into());
        }
        Ok(Self {
            root: root as u8,
            minor,
        })
    }
    pub fn from_key(key: Option<&str>) -> Self {
        key.and_then(|s| s.split_once(':'))
            .and_then(|(tonic, mode)| {
                ROOTS.iter().position(|&s| s == tonic).map(|root| Self {
                    root: root as u8,
                    minor: mode == "min",
                })
            })
            .unwrap_or_default()
    }
    fn steps(self) -> &'static [i16; 7] {
        if self.minor {
            &MINOR
        } else {
            &MAJOR
        }
    }
    fn label(self) -> String {
        format!(
            "{}:{}",
            ROOTS[usize::from(self.root)],
            if self.minor { "min" } else { "maj" }
        )
    }
}

#[derive(Clone, Copy)]
enum Mapping {
    Degree { number: usize, target: u8 },
    Chromatic,
    Outside(i16),
}
impl Mapping {
    fn reason(self) -> String {
        match self {
            Self::Degree { number, .. } => format!("degree {}", number + 1),
            Self::Chromatic => "chromatic source pitch".into(),
            Self::Outside(n) => format!("mapped MIDI {n} is outside 0–127"),
        }
    }
}
fn degree(note: i16, root: i16, scale: &[i16; 7]) -> Option<(i16, usize)> {
    let delta = note - root;
    let position = scale
        .iter()
        .position(|&step| step == delta.rem_euclid(12))?;
    Some((delta.div_euclid(12), position))
}
fn map(pitch: u8, src: Scale, dst: Scale) -> Mapping {
    let Some((octave, degree)) = degree(i16::from(pitch), i16::from(src.root), src.steps()) else {
        return Mapping::Chromatic;
    };
    // Anchor the target tonic nearest to the source tonic, preferring upward for a tritone.
    let distance = (i16::from(dst.root) - i16::from(src.root)).rem_euclid(12);
    let shift = if distance > 6 {
        distance - 12
    } else {
        distance
    };
    let target = i16::from(src.root) + shift + 12 * octave + dst.steps()[degree];
    if (0..=127).contains(&target) {
        Mapping::Degree {
            number: degree,
            target: target as u8,
        }
    } else {
        Mapping::Outside(target)
    }
}
fn named(p: u8) -> String {
    format!(
        "{}{} ({p})",
        ROOTS[usize::from(p % 12)],
        i16::from(p / 12) - 1
    )
}
fn tonal(e: &Event) -> bool {
    e.note.channel != 9 && !matches!(e.role, Role::Pad(_))
}
struct Row {
    mapping: Mapping,
    choice: Option<u8>,
    count: usize,
}
impl Row {
    fn target(&self) -> Option<u8> {
        match self.mapping {
            Mapping::Degree { target, .. } => Some(target),
            _ => self.choice,
        }
    }
}
pub(crate) struct Proposal {
    source: Scene,
    src: Scale,
    dst: Scale,
    rows: BTreeMap<u8, Row>,
    percussion: usize,
}
impl Proposal {
    pub fn new(scene: &Scene, src: Scale, dst: Scale) -> Result<Self, String> {
        scene.validate()?;
        let mut rows = BTreeMap::new();
        let mut percussion = 0;
        for ev in &scene.events {
            if tonal(ev) {
                let p = ev.note.pitch;
                rows.entry(p)
                    .or_insert_with(|| Row {
                        mapping: map(p, src, dst),
                        choice: None,
                        count: 0,
                    })
                    .count += 1;
            } else {
                percussion += 1;
            }
        }
        if rows.is_empty() {
            return Err("The scene contains no tonal notes.".into());
        }
        Ok(Self {
            source: scene.clone(),
            src,
            dst,
            rows,
            percussion,
        })
    }
    pub fn ready(&self) -> bool {
        self.rows.values().all(|v| v.target().is_some())
    }
    pub fn rows(&self) -> impl Iterator<Item = String> + '_ {
        self.rows.iter().map(|(&pitch, row)| {
            format!(
                "{} → {} · {} · {} event(s)",
                named(pitch),
                row.target().map_or_else(|| "choose a pitch".into(), named),
                row.mapping.reason(),
                row.count
            )
        })
    }
    fn issues(&self) -> impl Iterator<Item = (&u8, &Row)> {
        self.rows
            .iter()
            .filter(|(_, r)| !matches!(r.mapping, Mapping::Degree { .. }))
    }
    pub fn issue_labels(&self) -> impl Iterator<Item = String> + '_ {
        self.issues().map(|(&p, r)| {
            format!(
                "{} · {} · {}",
                named(p),
                r.mapping.reason(),
                r.choice.map_or_else(|| "decision required".into(), named)
            )
        })
    }
    pub fn resolve(&mut self, index: i32, choice: i32) -> Result<(), String> {
        let index = usize::try_from(index).map_err(|_| "Choose an unresolved pitch.")?;
        let pitch = self
            .issues()
            .nth(index)
            .map(|(&p, _)| p)
            .ok_or("Choose a pitch requiring a decision.")?;
        let target = match choice {
            -1 => pitch,
            0..=127 => choice as u8,
            _ => return Err("Select a MIDI pitch from 0 to 127.".into()),
        };
        if let Some(row) = self.rows.get_mut(&pitch) {
            row.choice = Some(target);
        }
        Ok(())
    }
    pub fn status(&self) -> String {
        let open = self.rows.values().filter(|r| r.target().is_none()).count();
        format!(
            "{} → {} · {} drum event(s) preserved · {open} pitch decision(s) outstanding.",
            self.src.label(),
            self.dst.label(),
            self.percussion
        )
    }
    pub fn result(&self, current: &Scene) -> Result<Scene, String> {
        if current != &self.source {
            return Err("The scene changed. Map the current notes again.".into());
        }
        if !self.ready() {
            return Err("Decide every chromatic or out-of-range pitch before continuing.".into());
        }
        let mut result = current.clone();
        for ev in &mut result.events {
            if tonal(ev) {
                ev.note.pitch = self
                    .rows
                    .get(&ev.note.pitch)
                    .and_then(Row::target)
                    .ok_or("Incomplete pitch mapping.")?;
            }
        }
        result.key = Some(self.dst.label());
        result.validate()?;
        Ok(result)
    }
}
