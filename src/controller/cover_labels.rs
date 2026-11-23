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
        let weak = Rc::downgrade(self);
        ui.on_cancel_cover_labels(move || {
            if let Some(c) = weak.upgrade() {
                c.close_reference_editor();
            }
        });
    }

    pub(super) fn open_cover_labels(&self, ui: &AppWindow) -> Result<(), String> {
        if ui.get_library_tab() != 4 || !self.storage_ready.get() {
            return Err("Open Prepare and wait for the library to finish loading".into());
        }
        self.close_sample_browser();
        self.close_comparison();
        self.close_reference_editor();
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
        let status = match (confirmed, estimate) {
            (Some(k), Some(a)) => format!("DJ-confirmed key: {k}. Estimated key: {a}."),
            (Some(k), None) => format!("DJ-confirmed key: {k}. No estimated key available."),
            (None, Some(a)) => format!("Estimated key: {a}. Save to confirm it for this track."),
            (None, None) => {
                "No estimated or confirmed key. Select a key to audition or confirm it.".into()
            }
        };
        self.begin_reference_editor(ui, track);
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
        if !self.reference_current(ui, id) {
            return Err("Track changed. Reopen its cover labels in Prepare.".into());
        }
        let labels = MusicLabels::parse(key, energy)?;
        self.write_music_labels(ui, id, labels)?;
        self.close_reference_editor();
        ui.set_status("Library cover labels updated".into());
        Ok(())
    }
    pub(super) fn write_music_labels(
        &self,
        ui: &AppWindow,
        id: i32,
        labels: MusicLabels,
    ) -> Result<(), String> {
        if self.library.borrow_mut().set_music_labels(id, labels)? {
            self.library_changed();
            let rows = ui.get_library_rows();
            if let Some(model) = rows.as_any().downcast_ref::<LibraryRows>() {
                if let Some(index) = model.rows.iter().position(|&visible| visible == id) {
                    model.notify.row_changed(index);
                }
            }
        }
        Ok(())
    }
}
