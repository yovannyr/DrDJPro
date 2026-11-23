//! Bind the active gig and the selected library track to the manual rule editor.
use super::*;
use crate::library::gigs::GigStatus;

impl AudioController {
    pub(super) fn wire_gigs(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_create_gig(move |name| {
            if let Some(c) = weak.upgrade() {
                c.finish_gig_action(c.create_gig(&name), "Gig created");
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_select_gig(move |index| {
            if let Some(c) = weak.upgrade() {
                c.finish_gig_action(c.select_gig(index), "Active gig updated");
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_save_gig_rule(move |track_id, status, reason| {
            if let Some(c) = weak.upgrade() {
                c.finish_gig_action(
                    c.save_gig_rule(track_id, &status, &reason),
                    "Gig rule updated",
                );
            }
        });
    }

    fn require_gig_storage(&self) -> Result<(), String> {
        if !self.storage_ready.get() || self.persistence.borrow().is_none() {
            return Err("Open a writable track library before changing gigs".into());
        }
        Ok(())
    }

    fn create_gig(&self, name: &str) -> Result<(), String> {
        self.require_gig_storage()?;
        self.library.borrow_mut().create_gig(name)?;
        Ok(())
    }

    fn gig_id_at(&self, index: i32) -> Result<Option<i32>, String> {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.gig_ids.borrow().get(index).copied())
            .ok_or_else(|| "Gig selection changed. Select the gig again.".into())
    }

    fn select_gig(&self, index: i32) -> Result<(), String> {
        self.require_gig_storage()?;
        let id = self.gig_id_at(index)?;
        self.library.borrow_mut().select_gig(id)
    }

    fn save_gig_rule(&self, id: i32, status: &str, reason: &str) -> Result<(), String> {
        self.require_gig_storage()?;
        let ui = self.ui.upgrade().ok_or("Window closed")?;
        let gig = self
            .gig_id_at(ui.get_gig_selection_index())?
            .ok_or("Select a gig first")?;
        let mut library = self.library.borrow_mut();
        if library.active_gig() != Some(gig)
            || id < 1
            || ui.get_selected_track() != id
            || ui.get_gig_rule_view().track_id != id
            || library.selected_id() != id
        {
            return Err("The gig or track changed. Select it again before saving.".into());
        }
        let status = if status.is_empty() {
            None
        } else {
            Some(GigStatus::parse(status).ok_or("Unknown gig rule")?)
        };
        library.set_gig_rule(gig, id, status, reason)
    }

    fn finish_gig_action(&self, result: Result<(), String>, message: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        match result {
            Ok(()) => {
                self.invalidate_suggestion();
                self.library_changed();
                self.publish_gigs(&ui, message);
                ui.set_status(message.into());
                self.save_library();
            }
            Err(error) => {
                self.publish_gig_rule(&ui, ui.get_selected_track(), &error);
                ui.set_status(error.into());
            }
        }
    }

    pub(super) fn invalidate_suggestion(&self) {
        if let Some(cancel) = self.suggestion_cancel.borrow_mut().take() {
            cancel.request();
        }
    }

    pub(super) fn publish_gigs(&self, ui: &AppWindow, notice: &str) {
        let library = self.library.borrow();
        let mut ids = vec![None];
        let mut names = vec![slint::SharedString::from("No active gig")];
        for (id, name) in library.gigs() {
            ids.push(Some(id));
            names.push(name.into());
        }
        let index = ids
            .iter()
            .position(|id| *id == library.active_gig())
            .unwrap_or(0);
        drop(library);
        *self.gig_ids.borrow_mut() = ids;
        ui.set_gig_names(ModelRc::new(Rc::new(VecModel::from(names))));
        ui.set_gig_selection_index(index as i32);
        self.publish_gig_rule(ui, ui.get_selected_track(), notice);
    }

    pub(super) fn publish_gig_rule(&self, ui: &AppWindow, track_id: i32, notice: &str) {
        let library = self.library.borrow();
        let rule = library
            .active_gig()
            .and_then(|gig| library.gig_rule(gig, track_id));
        let warning = match rule.filter(|rule| rule.status == GigStatus::Avoid) {
            Some(rule) if !rule.reason.is_empty() => format!(
                "Avoid for this gig. {} · Manual loading is available.",
                rule.reason
            )
            .into(),
            Some(_) => "Avoid for this gig. Manual loading is available.".into(),
            None => Default::default(),
        };
        let view = crate::GigRuleView {
            track_id,
            status: rule.map_or("", |r| r.status.as_str()).into(),
            reason: rule.map_or("", |r| r.reason.as_str()).into(),
            warning,
            fulfilled: rule.is_some_and(|r| r.fulfilled_at_ms.is_some()),
            notice: notice.into(),
        };
        drop(library);
        ui.set_gig_rule_view(view);
    }
}
