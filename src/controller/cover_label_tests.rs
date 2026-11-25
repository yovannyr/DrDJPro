#[test]
fn cover_labels_save_confirmed_values_for_both_layouts_without_writing_audio() {
    use sha2::{Digest, Sha256};
    i_slint_backend_testing::init_no_event_loop();
    let art = crate::cover_art::tests::embedded_cover_file();
    let plain = wav_file(0, 48_000);
    let paths = [
        art.path().canonicalize().unwrap(),
        plain.path().canonicalize().unwrap(),
    ];
    let hashes = || {
        paths
            .each_ref()
            .map(|p| Sha256::digest(std::fs::read(p).unwrap()))
    };
    let before = hashes();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    let (mixer, _out) = rodio::mixer::mixer(2, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000));
    let ui = AppWindow::new().unwrap();
    let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
    c.restore_library(Ok(db.clone()), paths.to_vec());
    finish_storage(&c);
    finish_import(&c);
    finish_storage(&c);
    ui.invoke_library_tab_selected(4);
    finish_search(&c);
    let ids = paths
        .each_ref()
        .map(|p| c.library.borrow().track_id_for_path(p).unwrap());
    let labels = |id| {
        let model = ui.get_library_rows();
        let row = model.iter().find(|row| row.id == id).unwrap();
        (row.cover_key.to_string(), row.cover_energy.to_string())
    };
    for (index, layout) in [crate::LayoutMode::Classic, crate::LayoutMode::Modern]
        .into_iter()
        .enumerate()
    {
        ui.set_view_mode(layout);
        ui.set_modern_library_open(true);
        let id = ids[index];
        let key = ["8A", "11B"][index];
        let energy = ["7", ""][index];
        ui.invoke_select_library(id);
        assert_eq!(ui.get_selected_track(), id);
        ui.invoke_open_cover_labels();
        assert_eq!(ui.get_cover_label_editor().track_id, id);
        ui.invoke_save_cover_labels(id, key.into(), "11".into());
        let editor = ui.get_cover_label_editor();
        assert!(editor.open);
        assert_eq!(editor.energy.as_str(), "11");
        assert!(!editor.status.is_empty());
        assert_eq!(labels(id), (String::new(), String::new()));
        ui.invoke_save_cover_labels(id, key.into(), energy.into());
        assert!(!ui.get_cover_label_editor().open);
        assert_eq!(labels(id), (key.to_owned(), energy.to_owned()));
        ui.invoke_open_cover_labels();
        ui.invoke_select_library(ids[1 - index]);
        ui.invoke_save_cover_labels(id, "3A".into(), "4".into());
        assert_eq!(labels(id), (key.to_owned(), energy.to_owned()));
        ui.invoke_cancel_cover_labels();
    }
    finish_storage(&c);
    c.close_library().unwrap();
    drop(c);
    let reopened = AppWindow::new().unwrap();
    let (mixer, _out) = rodio::mixer::mixer(2, 48_000);
    let restored = AudioController::wire(
        &reopened,
        Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000)),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    restored.restore_library(Ok(db), vec![]);
    finish_storage(&restored);
    for (index, id) in ids.into_iter().enumerate() {
        let library = restored.library.borrow();
        let value = library.track_by_id(id).unwrap().metadata.music_labels;
        assert_eq!(value.key().unwrap().to_string(), ["8A", "11B"][index]);
        assert_eq!(value.energy(), [Some(7), None][index]);
    }
    restored.close_library().unwrap();
    assert_eq!(hashes(), before);
}
