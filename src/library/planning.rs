use super::{
    gigs::GigStatus, Library, SearchRecord, SearchRecordChunks, Track, HISTORY_SCOPE, PREP_SCOPE,
    SEARCH_RECORD_CHUNK,
};
use crate::{
    hot_cues::HotCues,
    set_planner::{PlanningRef, PlanningTrack},
};
use drdjpro_ui_spike::analysis::keys::CamelotKey;
use std::{path::Path, sync::Arc, time::Duration};

const NO_CUE: u64 = u64::MAX;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct PlanningData {
    key: Option<CamelotKey>,
    bpm: f32,
    entry_bpm: f32,
    exit_bpm: f32,
    entry_ns: u64,
    exit_ns: u64,
}
impl PlanningData {
    pub(super) fn from_track(track: &Track) -> Self {
        let analysis = track.metadata.analysis.as_ref().filter(|a| !a.is_corrupt);
        let key = track
            .metadata
            .music_labels
            .key()
            .or_else(|| analysis.and_then(|a| CamelotKey::parse(&a.musical_key)));
        let bpm = track
            .grid
            .as_ref()
            .map(|g| g.bpm() as f32)
            .or_else(|| analysis.map(|a| a.bpm))
            .unwrap_or(0.);
        let (entry, exit) = cue_points(&track.hot_cues);
        let local_bpm = |point: Option<Duration>| {
            point
                .and_then(|at| track.grid.as_ref().map(|g| g.bpm_at_position(at) as f32))
                .unwrap_or(bpm)
        };
        Self {
            key,
            bpm,
            entry_bpm: local_bpm(entry),
            exit_bpm: local_bpm(exit),
            entry_ns: entry.map_or(NO_CUE, |t| t.as_nanos() as u64),
            exit_ns: exit.map_or(NO_CUE, |t| t.as_nanos() as u64),
        }
    }
}

fn cue_points(cues: &HotCues) -> (Option<Duration>, Option<Duration>) {
    let mut entry = None;
    let mut exit = None;
    let mut earliest = None;
    let mut latest = None;
    for cue in cues.slots().iter().flatten() {
        let t = cue.position();
        let name = cue.name().trim();
        if ["intro", "entry", "in", "einstieg"]
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
        {
            entry = Some(entry.map_or(t, |p: Duration| p.min(t)));
        } else if ["outro", "exit", "out", "ausstieg"]
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
        {
            exit = Some(exit.map_or(t, |p: Duration| p.max(t)));
        } else {
            earliest = Some(earliest.map_or(t, |p: Duration| p.min(t)));
            latest = Some(latest.map_or(t, |p: Duration| p.max(t)));
        }
    }
    let entry = entry.or(earliest);
    (entry, exit.or_else(|| latest.filter(|p| entry != Some(*p))))
}

#[derive(Clone)]
pub(crate) struct PlanningCatalog {
    tracks: SearchRecordChunks,
    excluded: Arc<Vec<i32>>,
    members: Option<Arc<Vec<u64>>>,
    smart_query: String,
}

impl SearchRecord {
    fn planning_ref(&self, avoid: bool, in_scope: bool) -> PlanningRef<'_> {
        let text = &self.search;
        let data = self.planning;
        let tempo = |v: f32| (v.is_finite() && v > 0.).then_some(f64::from(v));
        let cue = |v| (v != NO_CUE).then(|| Duration::from_nanos(v));
        PlanningRef {
            id: self.id,
            name: text.planning_title.as_deref().unwrap_or(if self.has_title {
                &text.title
            } else {
                &text.name
            }),
            path: text
                .native_path
                .as_deref()
                .unwrap_or_else(|| Path::new(&text.path)),
            key: data.key,
            bpm: tempo(data.bpm),
            entry_bpm: tempo(data.entry_bpm),
            exit_bpm: tempo(data.exit_bpm),
            entry: cue(data.entry_ns),
            exit: cue(data.exit_ns),
            available: !matches!(self.analysis_status, 1 | 2),
            avoid,
            in_scope,
        }
    }
}
impl PlanningCatalog {
    pub(crate) fn rows(&self) -> impl Iterator<Item = PlanningRef<'_>> {
        self.tracks
            .iter()
            .flat_map(|chunk| chunk.iter())
            .enumerate()
            .map(|(index, record)| {
                let in_scope = self.members.as_ref().is_none_or(|bits| {
                    bits.get(index / 64)
                        .is_some_and(|w| w & (1u64 << (index % 64)) != 0)
                }) && record.matches(&self.smart_query);
                record.planning_ref(self.excluded.binary_search(&record.id).is_ok(), in_scope)
            })
    }
}

impl Library {
    pub(super) fn update_planning_record(&mut self, index: usize) {
        let new = PlanningData::from_track(&self.tracks[index]);
        let chunk = index / SEARCH_RECORD_CHUNK;
        let offset = index % SEARCH_RECORD_CHUNK;
        if self.search_records[chunk][offset].planning != new {
            Arc::make_mut(&mut Arc::make_mut(&mut self.search_records)[chunk])[offset].planning =
                new;
        }
    }
    pub(crate) fn planning_track(&self, id: i32) -> Option<PlanningTrack> {
        let index = *self.id_indices.get(&id)?;
        let excluded = self
            .active_gig()
            .and_then(|gig| self.gig_rule(gig, id))
            .is_some_and(|r| r.status == GigStatus::Avoid);
        Some(
            self.search_records[index / SEARCH_RECORD_CHUNK][index % SEARCH_RECORD_CHUNK]
                .planning_ref(excluded, true)
                .owned(),
        )
    }
    /// Catalog records remain in shared 256-track chunks; snapshot only membership bits and active exclusions.
    pub(crate) fn planning_catalog(&self) -> PlanningCatalog {
        let mut excluded = Vec::new();
        if let Some(gig) = self.active_gig() {
            excluded.extend(
                self.gig_book
                    .rules
                    .range((gig, i32::MIN)..=(gig, i32::MAX))
                    .filter_map(|(&(_, id), rule)| (rule.status == GigStatus::Avoid).then_some(id)),
            );
        }
        let membership = |ids: &mut dyn Iterator<Item = &i32>| {
            let mut bits = vec![0u64; self.tracks.len().div_ceil(64)];
            for id in ids {
                if let Some(&i) = self.id_indices.get(id) {
                    bits[i / 64] |= 1u64 << (i % 64);
                }
            }
            Arc::new(bits)
        };
        let members = match self.active_crate {
            PREP_SCOPE => Some(membership(&mut self.prep_queue.iter())),
            HISTORY_SCOPE => Some(membership(&mut self.history.keys())),
            id if id != 0 && self.active_smart_query().is_none() => {
                Some(self.scope_bits.get(&id).cloned().unwrap_or_default())
            }
            _ => None,
        };
        PlanningCatalog {
            tracks: Arc::clone(&self.search_records),
            excluded: Arc::new(excluded),
            members,
            smart_query: self.active_smart_query().unwrap_or_default().to_owned(),
        }
    }
}
