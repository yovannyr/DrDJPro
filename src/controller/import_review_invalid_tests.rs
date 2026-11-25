fn open_exact_copy_review(ui: &AppWindow, directory: &Path) -> (Rc<AudioController>, PathBuf) {
    let (mixer, _output) = rodio::mixer::mixer(2, 48_000);
    let c = AudioController::wire(
        ui,
        Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000)),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    c.restore_library(Ok(directory.join("review.sqlite3")), vec![fixture()]);
    finish_storage(&c);
    finish_import(&c);
    finish_storage(&c);
    let copy = directory.join("exact-copy.mp3");
    std::fs::copy(fixture(), &copy).unwrap();
    let copy = copy.canonicalize().unwrap();
    c.import_paths(vec![copy.clone()]);
    finish_import(&c);
    assert!(ui.get_import_review_open());
    let rows = ui.get_import_review_rows();
    assert_eq!(rows.row_count(), 1);
    let row = rows.row_data(0).unwrap();
    assert!(row.eligible && !row.selected && row.matches.starts_with("Exact copy:"));
    (c, copy)
}

#[test]
fn import_review_keeps_rescan_choice_and_all_skipped_apply_preserves_revisions() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (c, copy) = open_exact_copy_review(&ui, directory.path());
    assert!(!c.library.borrow().analysis_targets().is_empty());
    assert!(c.pending_import.borrow().is_none());
    ui.invoke_analyze_library();
    assert_eq!(
        ui.get_status(),
        "Wait for the current operation before analyzing"
    );
    assert!(c.analysis_batch.borrow().is_none());
    assert!(c.pending_import.borrow().is_none());
    assert!(ui.get_import_review_open());

    let selected = || ui.get_import_review_rows().row_data(0).unwrap().selected;
    ui.invoke_toggle_import_review(0);
    assert!(selected());
    ui.invoke_rescan_import_review();
    assert!(ui.get_import_review_checking());
    finish_import(&c);
    assert!(ui.get_import_review_open() && !ui.get_import_review_checking());
    assert!(
        selected(),
        "unchanged evidence must preserve an explicit Keep"
    );
    ui.invoke_toggle_import_review(0);
    assert!(!selected());

    let revisions = || {
        let library = c.library.borrow();
        (
            library.revision(),
            library.search_spec(c.library_generation.get()).revision,
        )
    };
    let before = revisions();
    ui.invoke_apply_import_review();
    finish_import(&c);
    assert!(!ui.get_import_review_open());
    assert_eq!(ui.get_status(), "No tracks selected for import");
    assert_eq!(revisions(), before);
    assert_eq!(c.library.borrow().len(), 1);
    assert!(c.library.borrow().track_id_for_path(&copy).is_none());
    c.close_library().unwrap();
}

#[test]
fn import_review_closing_during_apply_discards_even_a_late_result() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (c, copy) = open_exact_copy_review(&ui, directory.path());
    ui.invoke_toggle_import_review(0);
    assert!(ui.get_import_review_rows().row_data(0).unwrap().selected);
    ui.invoke_apply_import_review();
    assert!(ui.get_import_review_checking() && c.pending_import.borrow().is_some());
    c.close_library().unwrap();
    assert!(!ui.get_import_review_open());
    finish_import(&c);
    assert_eq!(c.library.borrow().len(), 1);
    assert!(c.library.borrow().track_id_for_path(&copy).is_none());

    let store =
        crate::library_store::Worker::start(directory.path().join("review.sqlite3")).unwrap();
    let crate::library_store::Event::Loaded(Ok(saved)) = store.events.recv().unwrap() else {
        panic!("expected a saved library after shutdown during review");
    };
    assert_eq!(saved.library.len(), 1);
    assert!(saved.library.track_id_for_path(&copy).is_none());
    assert!(saved
        .library
        .track_id_for_path(&fixture().canonicalize().unwrap())
        .is_some());
    store.close_delta(None).unwrap();
}

#[test]
fn import_review_byte_identical_unreadable_audio_never_becomes_selectable() {
    use slint::Model;
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let (mixer, _output) = rodio::mixer::mixer(2, 48_000);
    let c = AudioController::wire(
        &ui,
        Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000)),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    c.restore_library(Ok(db.clone()), vec![]);
    finish_storage(&c);
    let invalid = dir.path().join("broken.mp3");
    let invalid_copy = dir.path().join("same-bytes.mp3");
    std::fs::write(&invalid, b"these are not audio frames").unwrap();
    std::fs::copy(&invalid, &invalid_copy).unwrap();
    let good = wav_file(123, 4096);
    c.import_paths(vec![
        invalid.clone(),
        invalid_copy.clone(),
        good.path().into(),
    ]);
    finish_import(&c);
    for pass in 0..2 {
        assert!(ui.get_import_review_open());
        let rows = ui.get_import_review_rows();
        assert_eq!(rows.row_count(), 3);
        assert_eq!(c.library.borrow().len(), 0);
        for row in rows.iter() {
            if std::path::Path::new(row.path.as_str()).extension().unwrap() == "mp3" {
                assert!(!row.selected && !row.eligible);
                assert!(
                    row.matches.contains("Not checked")
                        && row.matches.contains("No readable audio track"),
                    "{}",
                    row.matches
                );
                ui.invoke_toggle_import_review(row.index);
                assert!(!rows.row_data(row.index as usize).unwrap().selected);
            } else {
                assert!(row.selected && row.eligible);
            }
        }
        if pass == 0 {
            ui.invoke_rescan_import_review();
            finish_import(&c);
        }
    }
    ui.invoke_apply_import_review();
    finish_import(&c);
    assert!(!ui.get_import_review_open());
    finish_storage(&c);
    assert_eq!(c.library.borrow().len(), 1);
    assert!(c
        .library
        .borrow()
        .track_id_for_path(&good.path().canonicalize().unwrap())
        .is_some());
    assert_eq!(
        std::fs::read(invalid).unwrap(),
        std::fs::read(invalid_copy).unwrap()
    );
    c.close_library().unwrap();
}
