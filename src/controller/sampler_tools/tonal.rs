use super::super::set_planning::Lane;
use super::*;
use crate::sampler::tonal::Layout;

#[derive(Clone, PartialEq)]
struct Proposal {
    layout: Layout,
    correction: i32,
    token: u64,
    track: i32,
}
struct AuditionResult {
    proposal: Proposal,
    epoch: u64,
    bank: Arc<crate::sampler::Shared>,
    clip: PreviewClip,
}
#[derive(PartialEq, Eq)]
struct SourceState {
    reference: u8,
    slot: usize,
    track: Option<i32>,
    catalog_revision: u64,
    generation: Option<u64>,
    tuning: Option<(i32, i32)>,
}
#[derive(Default)]
pub(super) struct ScaleState {
    proposal: Option<Proposal>,
    audition: Lane<AuditionResult>,
    reference: u8,
    follow: Option<SourceState>,
}
impl AudioController {
    pub(super) fn wire_tonal_pads(self: &Rc<Self>, v: &SamplerToolsWindow) {
        let weak = Rc::downgrade(self);
        v.on_propose_scale(move |d| {
            if let Some(c) = weak.upgrade() {
                if (0..2).contains(&d) {
                    c.propose_scale(d as u8);
                }
            }
        });
        let weak = Rc::downgrade(self);
        v.on_audition_scale(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.audition_scale() {
                    c.sampler_tool_message(e);
                }
            }
        });
        let weak = Rc::downgrade(self);
        v.on_confirm_scale(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.confirm_scale() {
                    c.sampler_tool_message(e);
                } else {
                    c.sampler_tool_message("Tonal bank active".into());
                }
            }
        });
        let weak = Rc::downgrade(self);
        v.on_normal_pads(move || {
            if let Some(c) = weak.upgrade() {
                let mut s = c.sampler.borrow_mut();
                let _ = s.shared.use_layout(None);
                s.preferences.tonal = None;
                drop(s);
                c.sampler_tool_message("Normal pad bank restored".into());
            }
        });
    }
    fn current_proposal(&self, deck: u8) -> Result<Proposal, String> {
        let slot = self.sampler_tools.borrow().slot;
        let s = self.sampler.borrow();
        if s.job.is_some() || !s.restore.is_empty() {
            return Err("Finish preparing the sample first".into());
        }
        let token = s
            .shared
            .source_token(slot)
            .ok_or("Assign a tonal sample to this pad")?;
        let obs = s.observed[slot].ok_or("Sample analysis pending")?;
        let tuning = s.preferences.tuning[slot]
            .clone()
            .or_else(|| {
                obs.pitch.filter(|p| p.confidence >= 80).map(|p| Tuning {
                    root: p.midi,
                    correction: 0,
                    digest: obs.digest,
                })
            })
            .ok_or("Unknown root. Confirm a note in Tuning first.")?;
        let id = if deck == 0 { DeckId::A } else { DeckId::B };
        let path = self
            .engine
            .borrow()
            .snapshot(id)
            .path
            .ok_or("Load a reference track into the selected deck")?;
        let lib = self.library.borrow();
        let track = lib
            .track_id_for_path(&path)
            .ok_or("Reference track is not in the library")?;
        let key = lib
            .planning_track(track)
            .and_then(|t| t.key)
            .ok_or("Reference key unknown. Confirm the track key in the library first.")?;
        Ok(Proposal {
            layout: Layout::make(slot as u8, &tuning, deck, key)?,
            correction: tuning.correction,
            token,
            track,
        })
    }
    fn propose_scale(&self, deck: u8) {
        let result = self.current_proposal(deck);
        let mut t = self.sampler_tools.borrow_mut();
        t.scale.audition.cancel();
        t.scale.reference = deck;
        let msg = match &result {
            Ok(p) => format!(
                "Deck {} · {} · pads 1–4: {}",
                if deck == 0 { "A" } else { "B" },
                p.layout.key,
                p.layout.notes.map(|n| note_name(i32::from(n))).join(" · ")
            ),
            Err(e) => e.clone(),
        };
        if let Some(v) = &t.view {
            v.set_scale_ready(result.is_ok());
            v.set_scale_description(msg.into());
        }
        t.scale.proposal = result.ok();
    }
    fn checked_scale(&self) -> Result<Proposal, String> {
        let draft = self
            .sampler_tools
            .borrow()
            .scale
            .proposal
            .clone()
            .ok_or("Select a reference deck first")?;
        let current = self.current_proposal(draft.layout.deck)?;
        if current != draft {
            return Err("Sample or reference changed. Propose the current key again.".into());
        }
        Ok(draft)
    }
    fn confirm_scale(&self) -> Result<(), String> {
        let p = self.checked_scale()?;
        let mut s = self.sampler.borrow_mut();
        if !s.writable {
            return Err("Sampler settings are read-only".into());
        }
        s.shared.use_layout(Some(&p.layout))?;
        s.preferences.tonal = Some(p.layout);
        Ok(())
    }
    fn audition_scale(&self) -> Result<(), String> {
        let p = self.checked_scale()?;
        let e = self.engine.borrow();
        if !e.preview_route_available() {
            return Err("Choose a separate headphone pair (3/4)".into());
        }
        let epoch = e.preview_epoch();
        let bank = e.sampler();
        self.sampler_tools
            .borrow_mut()
            .scale
            .audition
            .submit(Box::new(move |cancel| {
                let clip = bank.scale_clip(&p.layout, p.token, cancel)?;
                Ok(AuditionResult {
                    proposal: p,
                    epoch,
                    bank,
                    clip,
                })
            }));
        Ok(())
    }
    pub(super) fn poll_tonal_pads(&self) {
        let mut t = self.sampler_tools.borrow_mut();
        let Some(v) = t.view.as_ref() else {
            return;
        };
        let s = self.sampler.borrow();
        let active = s.preferences.tonal.as_ref().map_or_else(
            || "Normal pads active".to_owned(),
            |l| {
                format!(
                    "Active · Deck {} · {}",
                    if l.deck == 0 { "A" } else { "B" },
                    l.key
                )
            },
        );
        if v.get_active_scale().as_str() != active {
            v.set_active_scale(active.into());
        }
        let side = t.scale.reference;
        let slot = t.slot;
        let stamp = SourceState {
            reference: side,
            slot,
            track: self.deck_session.borrow().decks[side as usize],
            catalog_revision: self.library.borrow().revision(),
            generation: s.shared.source_token(slot),
            tuning: s.preferences.tuning[slot]
                .as_ref()
                .map(|x| (x.root, x.correction)),
        };
        drop(s);
        let changed = t.scale.follow.as_ref() != Some(&stamp);
        t.scale.follow = Some(stamp);
        let completed = t.scale.audition.poll();
        drop(t);
        if changed {
            self.propose_scale(side);
        }
        if let Some(result) = completed {
            let result = result.and_then(|ready| {
                if self.checked_scale()? != ready.proposal {
                    return Err("Preview source changed".into());
                }
                let e = self.engine.borrow();
                if e.preview_epoch() != ready.epoch || !Arc::ptr_eq(&ready.bank, &e.sampler()) {
                    return Ok(());
                }
                e.play_sample_preview(&ready.clip)?;
                self.sampler_tools.borrow_mut().preview =
                    Some((Rc::downgrade(&e), e.preview_epoch()));
                Ok(())
            });
            if let Err(e) = result {
                self.sampler_tool_message(e);
            }
        }
    }
}
