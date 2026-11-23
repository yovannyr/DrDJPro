use super::*;
use crate::track_metadata::MusicLabels;
use drdjpro_ui_spike::analysis::keys::CamelotKey;

impl AudioController {
    pub(super) fn wire_cover_labels(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_cover_labels(move || {
            let Some(c) = weak.upgrade() else { return };
            let Some(ui) = c.ui.upgrade() else { return };
            if let Err(error) = c.open_cover_labels(&ui) {
                ui.set_status(error.into());
            }
        });
        let weak = Rc::downgrade(self);
        ui.on_save_cover_labels(move |id, key, energy| {
            let Some(c) = weak.upgrade() else { return };
            let Some(ui) = c.ui.upgrade() else { return };
            if let Err(error) = c.save_cover_labels(&ui, id, &key, &energy) {
                let mut editor = ui.get_cover_label_editor();
                if editor.open && editor.track_id == id && ui.get_selected_track() == id {
                    editor.key = key;
                    editor.energy = energy;
                    editor.status = error.clone().into();
                    ui.set_cover_label_editor(editor);
                }
                ui.set_status(error.into());
            }
        });
        let view = ui.as_weak();
        ui.on_cancel_cover_labels(move || {
            if let Some(ui) = view.upgrade() {
                ui.set_cover_label_editor(Default::default());
            }
        });
    }

    fn open_cover_labels(&self, ui: &AppWindow) -> Result<(), String> {
        if ui.get_library_tab() != 4 || !self.storage_ready.get() {
            return Err("Open Prepare and wait for the library to finish loading".into());
        }
        self.close_comparison();
        let library = self.library.borrow();
        let id = library.selected_id();
        if id < 1 || id != ui.get_selected_track() {
            return Err("Select a track in Prepare first".into());
        }
        let track = library.track_by_id(id).ok_or("Track no longer available")?;
        let labels = &track.metadata.music_labels;
        let confirmed = labels.key();
        let estimate = track
            .metadata
            .analysis
            .as_ref()
            .and_then(|analysis| CamelotKey::parse(&analysis.musical_key));
        let status = if confirmed.is_none() && estimate.is_some() {
            "Estimated key. Save to confirm it for this track."
        } else {
            "Leave a field empty to clear its label."
        };
        ui.set_cover_label_editor(crate::CoverLabelEditor {
            open: true,
            track_id: id,
            key: confirmed
                .or(estimate)
                .map(|key| key.to_string())
                .unwrap_or_default()
                .into(),
            energy: labels
                .energy()
                .map(|value| value.to_string())
                .unwrap_or_default()
                .into(),
            status: status.into(),
        });
        Ok(())
    }

    fn save_cover_labels(
        &self,
        ui: &AppWindow,
        id: i32,
        key: &str,
        energy: &str,
    ) -> Result<(), String> {
        let editor = ui.get_cover_label_editor();
        if !editor.open
            || editor.track_id != id
            || ui.get_library_tab() != 4
            || ui.get_selected_track() != id
            || self.library.borrow().selected_id() != id
        {
            return Err("Track changed. Reopen its cover labels in Prepare.".into());
        }
        if !self.storage_ready.get() {
            return Err("Wait for the library to finish loading".into());
        }
        let labels = MusicLabels::parse(key, energy)?;
        if self.library.borrow_mut().set_music_labels(id, labels)? {
            self.library_changed();
            let rows = ui.get_library_rows();
            if let Some(model) = rows.as_any().downcast_ref::<LibraryRows>() {
                if let Some(index) = model.rows.iter().position(|&visible| visible == id) {
                    model.notify.row_changed(index);
                }
            }
        }
        ui.set_cover_label_editor(Default::default());
        ui.set_status("Library cover labels updated".into());
        Ok(())
    }
}
