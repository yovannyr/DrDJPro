use super::*;
use crate::{
    midi_scene::transpose::{Proposal, Scale},
    SceneWindow,
};

#[cfg(test)]
mod acceptance;

#[derive(Default)]
pub(super) struct ScaleState {
    source: Scale,
    proposal: Option<Proposal>,
    notice: String,
}
fn labels(items: impl Iterator<Item = String>) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(items.map(Into::into).collect::<Vec<_>>()))
}
impl ScaleState {
    pub(super) fn reset(&mut self, key: Option<&str>) {
        self.source = Scale::from_key(key);
        self.clear();
    }
    pub(super) fn clear(&mut self) {
        self.proposal = None;
        self.notice.clear();
    }
    pub(super) fn publish(&self, v: &SceneWindow) {
        v.set_scale_source_root(i32::from(self.source.root));
        v.set_scale_source_minor(self.source.minor);
        v.set_scale_active(self.proposal.is_some());
        v.set_scale_ready(self.proposal.as_ref().is_some_and(Proposal::ready));
        if let Some(p) = &self.proposal {
            v.set_scale_rows(labels(p.rows()));
            v.set_scale_issues(labels(p.issue_labels()));
            v.set_scale_status(if self.notice.is_empty() {
                p.status().into()
            } else {
                self.notice.as_str().into()
            });
        } else {
            v.set_scale_rows(Default::default());
            v.set_scale_issues(Default::default());
            v.set_scale_status(if self.notice.is_empty() {
                "Choose source and target scales, then map the notes.".into()
            } else {
                self.notice.as_str().into()
            });
        }
    }
}
impl AudioController {
    pub(super) fn wire_scene_scale(self: &Rc<Self>, v: &SceneWindow) {
        macro_rules! bind{
            ($cb:ident,$method:ident $(,$arg:ident)*)=>{{
                let weak=Rc::downgrade(self);v.$cb(move|$($arg),*|{
                    if let Some(c)=weak.upgrade(){
                        let result=c.$method($($arg),*);
                        let mut s=c.scene.borrow_mut();if let Err(e)=result{s.scale.notice=e;}
                        if let Some(v)=&s.view{s.scale.publish(v);}
                    }
                });
            }}
        }
        bind!(
            on_propose_scale,
            propose_scene_scale,
            from,
            minor,
            to,
            target_minor
        );
        bind!(on_resolve_scale, resolve_scene_scale, index, pitch);
        bind!(on_discard_scale, discard_scene_scale);
        bind!(on_accept_scale, accept_scene_scale);
        bind!(on_hear_scale, hear_scene_scale, proposed);
    }
    fn propose_scene_scale(
        &self,
        from: i32,
        minor: bool,
        to: i32,
        target_minor: bool,
    ) -> Result<(), String> {
        let mut s = self.scene.borrow_mut();
        s.stop_audio();
        s.scale.clear();
        let p = Proposal::new(
            &s.draft,
            Scale::new(from, minor)?,
            Scale::new(to, target_minor)?,
        )?;
        s.scale.proposal = Some(p);
        Ok(())
    }
    fn resolve_scene_scale(&self, index: i32, pitch: i32) -> Result<(), String> {
        let mut s = self.scene.borrow_mut();
        s.stop_audio();
        s.scale
            .proposal
            .as_mut()
            .ok_or("Map the current scene first.")?
            .resolve(index, pitch)?;
        s.scale.notice.clear();
        Ok(())
    }
    fn discard_scene_scale(&self) -> Result<(), String> {
        let mut s = self.scene.borrow_mut();
        s.stop_audio();
        s.scale.clear();
        Ok(())
    }
    fn accept_scene_scale(&self) -> Result<(), String> {
        let mut s = self.scene.borrow_mut();
        let next = s
            .scale
            .proposal
            .as_ref()
            .ok_or("Map the current scene first.")?
            .result(&s.draft)?;
        s.replace(
            next,
            "Scale mapping applied. Undo restores the previous scene.",
        );
        Ok(())
    }
    fn hear_scene_scale(&self, proposed: bool) -> Result<(), String> {
        let s = self.scene.borrow();
        let preview = if proposed {
            s.scale
                .proposal
                .as_ref()
                .ok_or("Map the current scene first.")?
                .result(&s.draft)?
        } else {
            s.draft.clone()
        };
        drop(s);
        self.audition_scene_notes(&preview)
    }
}
