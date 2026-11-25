fn write_track(dir: &Path, file: &str, title: &str) -> PathBuf {
    let fixture = crate::test_support::tagged_mp3(title, "DJ Test", "Set");
    let dest = dir.join(file);
    std::fs::copy(fixture.path(), &dest).unwrap();
    std::fs::canonicalize(&dest).unwrap()
}

fn wait_for(c: &AudioController, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        c.tick();
        assert!(
            Instant::now() < deadline,
            "import decision timed out: {}",
            c.ui.upgrade().unwrap().get_status()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn choose_import(ui: &AppWindow, file: &str, keep: bool) {
    let row = ui
        .get_import_review_rows()
        .iter()
        .find(|row| row.path.as_str().ends_with(file))
        .unwrap();
    if row.selected != keep {
        ui.invoke_toggle_import_review(row.index);
    }
    let row = ui
        .get_import_review_rows()
        .iter()
        .find(|row| row.path.as_str().ends_with(file))
        .unwrap();
    assert_eq!(row.selected, keep);
}

#[test]
fn import_review_files_and_folder_preserve_crate_cues_and_bytes_until_approved() {
    use sha2::{Digest, Sha256};
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let incoming = dir.path().join("incoming");
    std::fs::create_dir(&incoming).unwrap();
    let original = write_track(dir.path(), "Original.mp3", "Signal (Radio Edit)");
    let copy = incoming.join("Copy.mp3");
    std::fs::copy(&original, &copy).unwrap();
    let copy = copy.canonicalize().unwrap();
    let extended = write_track(&incoming, "Long.mp3", "Signal (Extended Mix)");
    let hashes =
        || [&original, &copy, &extended].map(|p| Sha256::digest(std::fs::read(p).unwrap()));
    let before = hashes();
    let paths = vec![copy.clone(), extended.clone()];
    let ui = AppWindow::new().unwrap();
    let (mixer, _output) = rodio::mixer::mixer(2, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000));
    let c = AudioController::wire(
        &ui,
        engine,
        Arc::new(|| None),
        Arc::new(move || Some(paths.clone())),
    );
    c.restore_library(Ok(dir.path().join("db.sqlite3")), vec![original.clone()]);
    finish_storage(&c);
    finish_import(&c);
    let cue =
        crate::hot_cues::HotCue::new(Duration::from_secs(1), "Keep".into(), 0x123456).unwrap();
    let (original_id, crate_id) = {
        let mut lib = c.library.borrow_mut();
        let id = lib.track_id_for_path(&original).unwrap();
        lib.set_hot_cue(&original, 0, Some(cue.clone())).unwrap();
        let crate_id = lib.create_crate("Saturday").unwrap();
        lib.add_paths_to_crate(crate_id, std::slice::from_ref(&original))
            .unwrap();
        lib.select_crate(crate_id).unwrap();
        (id, crate_id)
    };
    c.library_changed();
    c.publish_library();
    finish_search(&c);
    let await_review = || {
        wait_for(&c, || {
            ui.get_import_review_open() && !ui.get_import_review_checking()
        })
    };
    let assert_uncommitted = || {
        let lib = c.library.borrow();
        assert_eq!(lib.track_id_for_path(&original), Some(original_id));
        assert!(lib.track_id_for_path(&copy).is_none());
        assert!(lib.track_id_for_path(&extended).is_none());
        assert_eq!(lib.hot_cues(&original).slots()[0], Some(cue.clone()));
        let tracks = &lib
            .crates()
            .entries
            .iter()
            .find(|entry| entry.id == crate_id)
            .unwrap()
            .tracks;
        assert_eq!(tracks.len(), 1);
        assert!(tracks.contains(&original_id));
    };
    ui.invoke_import_library();
    await_review();
    assert_eq!(ui.get_import_review_rows().row_count(), 2);
    let row = ui
        .get_import_review_rows()
        .iter()
        .find(|r| r.path.as_str().ends_with("Copy.mp3"))
        .unwrap();
    assert!(row.matches.contains("Original.mp3"), "{}", row.matches);
    assert_uncommitted();
    choose_import(&ui, "Copy.mp3", true);
    ui.invoke_apply_import_review();
    assert!(ui.get_import_review_checking());
    ui.invoke_cancel_import_review();
    wait_for(&c, || c.pending_import.borrow().is_none());
    assert!(!ui.get_import_review_open());
    assert_uncommitted();
    c.set_library_folder_picker(Arc::new(move || Some(incoming.clone())));
    ui.invoke_import_library_folder();
    await_review();
    ui.invoke_rescan_import_review();
    await_review();
    assert_uncommitted();
    choose_import(&ui, "Copy.mp3", true);
    std::fs::copy(&extended, &copy).unwrap();
    ui.invoke_apply_import_review();
    await_review();
    assert!(ui.get_import_review_status().contains("changed"));
    assert_uncommitted();
    std::fs::copy(&original, &copy).unwrap();
    ui.invoke_rescan_import_review();
    await_review();
    choose_import(&ui, "Copy.mp3", false);
    choose_import(&ui, "Long.mp3", true);
    ui.invoke_apply_import_review();
    wait_for(&c, || {
        !ui.get_import_review_open() && c.library.borrow().track_id_for_path(&extended).is_some()
    });
    {
        let lib = c.library.borrow();
        let edit_id = lib.track_id_for_path(&extended).unwrap();
        assert!(lib.track_id_for_path(&copy).is_none());
        let tracks = &lib
            .crates()
            .entries
            .iter()
            .find(|entry| entry.id == crate_id)
            .unwrap()
            .tracks;
        assert!(tracks.contains(&original_id) && tracks.contains(&edit_id));
        assert_eq!(lib.hot_cues(&original).slots()[0], Some(cue.clone()));
    }
    assert_eq!(hashes(), before);
    c.close_library().unwrap();
    let store = crate::library_store::Worker::start(dir.path().join("db.sqlite3")).unwrap();
    let crate::library_store::Event::Loaded(Ok(saved)) = store.events.recv().unwrap() else {
        panic!("expected the persisted import");
    };
    let reopened = &saved.library;
    assert_eq!(reopened.hot_cues(&original).slots()[0], Some(cue));
    assert!(reopened.track_id_for_path(&copy).is_none());
    let edit_id = reopened.track_id_for_path(&extended).unwrap();
    let entries = &reopened.crates().entries;
    let members = &entries
        .iter()
        .find(|entry| entry.id == crate_id)
        .unwrap()
        .tracks;
    assert_eq!(members.len(), 2);
    assert!(members.contains(&original_id) && members.contains(&edit_id));
    store.close_delta(None).unwrap();
}
