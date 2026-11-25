//! Per-gig track rules. Fulfilment comes only from a confirmed playback entry.
use super::Library;
use std::collections::{BTreeMap, HashSet};

pub(crate) const MAX_GIGS: usize = 256;
pub(crate) const MAX_RULES: usize = 32768;
pub(crate) const NAME_BYTES: usize = 128;
pub(crate) const REASON_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GigStatus {
    Required,
    Request,
    Avoid,
}
impl GigStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Request => "request",
            Self::Avoid => "avoid",
        }
    }
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "required" => Some(Self::Required),
            "request" => Some(Self::Request),
            "avoid" => Some(Self::Avoid),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GigRule {
    pub status: GigStatus,
    pub reason: String,
    pub fulfilled_at_ms: Option<u64>,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct GigBook {
    pub names: BTreeMap<i32, String>,
    pub active: Option<i32>,
    pub rules: BTreeMap<(i32, i32), GigRule>,
}

pub(crate) fn valid_name(s: &str) -> bool {
    !s.is_empty() && s == s.trim() && s.len() <= NAME_BYTES && !s.chars().any(char::is_control)
}
impl GigRule {
    pub(crate) fn valid(&self) -> bool {
        self.reason.len() <= REASON_BYTES
            && self.reason.trim() == self.reason
            && !self.reason.chars().any(char::is_control)
            && self
                .fulfilled_at_ms
                .is_none_or(|ts| ts <= i64::MAX as u64 && self.status == GigStatus::Required)
    }
}
impl GigBook {
    pub(super) fn validate(&self, tracks: &HashSet<i32>) -> Result<(), String> {
        let mut names = HashSet::new();
        if self.names.len() > MAX_GIGS
            || self.names.iter().any(|(&id, name)| {
                id <= 0
                    || id == i32::MAX
                    || !valid_name(name)
                    || !names.insert(name.to_ascii_lowercase())
            })
        {
            return Err("Invalid or duplicate gig name or ID".into());
        }
        if self.active.is_some_and(|id| !self.names.contains_key(&id)) {
            return Err("Active gig does not exist".into());
        }
        if self.rules.len() > MAX_RULES
            || self.rules.iter().any(|((gig, track), rule)| {
                !self.names.contains_key(gig) || !tracks.contains(track) || !rule.valid()
            })
        {
            return Err("Invalid gig rule or unknown track".into());
        }
        Ok(())
    }
}

/// Only changed rows travel to the store, even for an otherwise large library.
#[derive(Default)]
pub(crate) struct GigDelta {
    pub names: Vec<(i32, String)>,
    pub selection: Option<Option<i32>>,
    pub rules: Vec<((i32, i32), Option<GigRule>)>,
}
impl GigDelta {
    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty() && self.selection.is_none() && self.rules.is_empty()
    }
}

#[derive(Default)]
pub(super) struct GigChanges {
    names: BTreeMap<i32, u64>,
    selection: Option<u64>,
    rules: BTreeMap<(i32, i32), u64>,
    revision: u64,
}
impl GigChanges {
    pub(super) fn has_changes(&self) -> bool {
        !self.names.is_empty() || self.selection.is_some() || !self.rules.is_empty()
    }
    pub(super) fn delta(&self, book: &GigBook) -> GigDelta {
        GigDelta {
            names: self
                .names
                .keys()
                .map(|id| (*id, book.names[id].clone()))
                .collect(),
            selection: self.selection.map(|_| book.active),
            rules: self
                .rules
                .keys()
                .map(|id| (*id, book.rules.get(id).cloned()))
                .collect(),
        }
    }
    pub(super) fn acknowledge(&mut self, revision: u64) {
        self.names.retain(|_, r| *r > revision);
        self.rules.retain(|_, r| *r > revision);
        if self.selection.is_some_and(|r| r <= revision) {
            self.selection = None;
        }
    }
}

impl Library {
    pub(crate) fn gigs(&self) -> impl Iterator<Item = (i32, &str)> {
        self.gig_book
            .names
            .iter()
            .map(|(&id, name)| (id, name.as_str()))
    }
    pub(crate) fn active_gig(&self) -> Option<i32> {
        self.gig_book.active
    }
    pub(crate) fn gig_rule(&self, gig_id: i32, track_id: i32) -> Option<&GigRule> {
        self.gig_book.rules.get(&(gig_id, track_id))
    }
    pub(crate) fn gig_revision(&self) -> u64 {
        self.gig_dirty.revision
    }

    fn touch_gigs(&mut self) -> u64 {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("Library revision overflow");
        self.gig_dirty.revision = self.revision;
        self.revision
    }
    pub(crate) fn create_gig(&mut self, name: &str) -> Result<i32, String> {
        let name = name.trim();
        if !valid_name(name) {
            return Err("Enter a gig name of 1–128 UTF-8 bytes without control characters".into());
        }
        if self
            .gig_book
            .names
            .values()
            .any(|n| n.eq_ignore_ascii_case(name))
        {
            return Err("A gig with this name already exists".into());
        }
        if self.gig_book.names.len() >= MAX_GIGS {
            return Err("Gig limit reached".into());
        }
        let id = self
            .gig_book
            .names
            .last_key_value()
            .map_or(1, |(&last, _)| last.saturating_add(1));
        if id == i32::MAX {
            return Err("Gig ID limit reached".into());
        }
        let revision = self.touch_gigs();
        self.gig_book.names.insert(id, name.into());
        self.gig_book.active = Some(id);
        self.gig_dirty.names.insert(id, revision);
        self.gig_dirty.selection = Some(revision);
        Ok(id)
    }
    pub(crate) fn select_gig(&mut self, selection: Option<i32>) -> Result<(), String> {
        if selection.is_some_and(|id| !self.gig_book.names.contains_key(&id)) {
            return Err("Select an existing gig".into());
        }
        if selection != self.gig_book.active {
            self.gig_book.active = selection;
            self.gig_dirty.selection = Some(self.touch_gigs());
        }
        Ok(())
    }
    pub(crate) fn set_gig_rule(
        &mut self,
        gig_id: i32,
        track_id: i32,
        status: Option<GigStatus>,
        reason: &str,
    ) -> Result<(), String> {
        if !self.gig_book.names.contains_key(&gig_id) || !self.id_indices.contains_key(&track_id) {
            return Err("Select an existing gig and track".into());
        }
        let key = (gig_id, track_id);
        if let Some(status) = status {
            let reason = reason.trim();
            if reason.len() > REASON_BYTES || reason.chars().any(char::is_control) {
                return Err(
                    "Gig reason exceeds 512 UTF-8 bytes or contains control characters".into(),
                );
            }
            let previous = self.gig_book.rules.get(&key);
            if previous.is_some_and(|r| r.status == status && r.reason == reason) {
                return Ok(());
            }
            if previous.is_none() && self.gig_book.rules.len() >= MAX_RULES {
                return Err("Gig rule limit reached".into());
            }
            let fulfilled_at_ms = previous
                .filter(|r| r.status == status)
                .and_then(|r| r.fulfilled_at_ms);
            self.gig_book.rules.insert(
                key,
                GigRule {
                    status,
                    reason: reason.into(),
                    fulfilled_at_ms,
                },
            );
        } else if self.gig_book.rules.remove(&key).is_none() {
            return Ok(());
        }
        let revision = self.touch_gigs();
        self.gig_dirty.rules.insert(key, revision);
        Ok(())
    }
    /// Uses the revision of the same record_play call so both rows are sent in a single delta.
    pub(super) fn fulfill_gig(&mut self, track_id: i32, played_at_ms: u64) {
        let Some(active) = self.gig_book.active else {
            return;
        };
        let key = (active, track_id);
        if let Some(rule) = self.gig_book.rules.get_mut(&key) {
            if rule.status == GigStatus::Required && rule.fulfilled_at_ms.is_none() {
                rule.fulfilled_at_ms = Some(played_at_ms);
                self.gig_dirty.rules.insert(key, self.revision);
                self.gig_dirty.revision = self.revision;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gig_rules_scope_fulfillment_to_the_active_gig_and_preserve_the_first_confirmed_play() {
        let mut lib = Library::default();
        lib.insert([crate::test_support::fixture()]);
        lib.record_play(1, 10, 0).unwrap();
        let a = lib.create_gig("A").unwrap();
        lib.set_gig_rule(a, 1, Some(GigStatus::Required), "first crowd")
            .unwrap();
        let b = lib.create_gig("B").unwrap();
        lib.set_gig_rule(b, 1, Some(GigStatus::Required), "encore")
            .unwrap();
        assert!(lib.create_gig(" a ").is_err());
        let open = lib.changes();
        assert!(lib.gig_rule(b, 1).unwrap().fulfilled_at_ms.is_none());
        lib.record_play(1, 20, 0).unwrap();
        lib.acknowledge(open.revision);
        let delta = lib.changes();
        assert_eq!(delta.gigs.rules.len(), 1);
        assert_eq!(delta.history.len(), 1);
        assert!(!delta.gigs.rules[0].1.as_ref().unwrap().reason.is_empty());
        assert!(lib.gig_rule(a, 1).unwrap().fulfilled_at_ms.is_none());
        let rev = lib.gig_revision();
        lib.record_play(1, 30, 1).unwrap();
        assert_eq!(rev, lib.gig_revision());
        lib.set_gig_rule(b, 1, Some(GigStatus::Required), "new reason")
            .unwrap();
        assert_eq!(lib.gig_rule(b, 1).unwrap().fulfilled_at_ms, Some(20));
        lib.set_gig_rule(b, 1, Some(GigStatus::Request), "")
            .unwrap();
        lib.set_gig_rule(b, 1, Some(GigStatus::Required), "")
            .unwrap();
        assert!(lib.gig_rule(b, 1).unwrap().fulfilled_at_ms.is_none());
        lib.select_gig(None).unwrap();
        lib.record_play(1, 40, 1).unwrap();
        assert!(lib.gig_rule(b, 1).unwrap().fulfilled_at_ms.is_none());
        assert!(lib
            .set_gig_rule(b, 1, Some(GigStatus::Request), &"ü".repeat(257))
            .is_err());
        assert!(lib
            .set_gig_rule(b, 99, Some(GigStatus::Request), "")
            .is_err());
        lib.set_gig_rule(b, 1, None, "").unwrap();
        assert!(lib.gig_rule(b, 1).is_none());
        assert!(lib.gig_rule(a, 1).is_some());
        assert_eq!(lib.gigs().count(), 2);
    }
}
