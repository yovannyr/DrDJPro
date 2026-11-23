fn choose_gig(ui: &AppWindow, name: &str) {
    let index = ui
        .get_gig_names()
        .iter()
        .position(|entry| entry.as_str() == name)
        .unwrap();
    let index = i32::try_from(index).unwrap();
    ui.invoke_select_gig(index);
    assert_eq!(ui.get_gig_selection_index(), index);
}

#[test]
fn gig_rules_survive_restart_and_only_confirmed_play_fulfils_the_active_request() {
    use crate::library::gigs::GigStatus;
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("gigs.sqlite3");
    let path = fixture().canonicalize().unwrap();
    let open = |initial: Vec<PathBuf>| {
        let (mixer, output) = rodio::mixer::mixer(2, 48_000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000));
        let ui = AppWindow::new().unwrap();
        let c = AudioController::wire(
            &ui,
            Rc::clone(&engine),
            Arc::new(|| None),
            Arc::new(|| None),
        );
        c.restore_library(Ok(db.clone()), initial);
        finish_storage(&c);
        finish_import(&c);
        finish_storage(&c);
        ui.invoke_library_tab_selected(4);
        finish_search(&c);
        (c, ui, engine, output)
    };
    let (c, ui, engine, output) = open(vec![path.clone()]);
    let id = c.library.borrow().track_id_for_path(&path).unwrap();
    ui.invoke_select_library(id);
    c.library.borrow_mut().record_play(id, 100, 0).unwrap();
    c.library_changed();
    ui.invoke_create_gig("Gig A".into());
    let a = c.library.borrow().active_gig().unwrap();
    ui.invoke_save_gig_rule(id, "avoid".into(), "Reserved for the other room".into());
    assert_eq!(ui.get_gig_rule_view().status, "avoid");
    assert!(!ui.get_gig_rule_view().warning.is_empty());
    ui.invoke_create_gig("Gig B".into());
    let b = c.library.borrow().active_gig().unwrap();
    assert_ne!(a, b);
    ui.invoke_save_gig_rule(id, "required".into(), "Opening request".into());
    assert!(!ui.get_gig_rule_view().fulfilled);
    c.close_library().unwrap();
    drop((c, ui, engine, output));

    let (c, ui, engine, mut output) = open(vec![]);
    assert_eq!(c.library.borrow().active_gig(), Some(b));
    ui.invoke_select_library(id);
    assert_eq!(ui.get_gig_rule_view().status, "required");
    assert!(!ui.get_gig_rule_view().fulfilled);
    choose_gig(&ui, "Gig A");
    ui.invoke_library_tab_selected(0);
    finish_search(&c);
    ui.invoke_select_library(id);
    assert!(!ui.get_gig_rule_view().warning.is_empty());
    ui.invoke_load_library_left();
    finish(&c);
    assert_eq!(engine.snapshot(DeckId::A).path.as_deref(), Some(path.as_path()));
    assert!(!engine.snapshot(DeckId::A).playing);

    ui.invoke_library_tab_selected(4);
    choose_gig(&ui, "Gig B");
    ui.invoke_monitor_cue_changed(0);
    ui.invoke_temporary_cue(0, true, false);
    assert!(engine.snapshot(DeckId::A).playing);
    output.by_ref().take(960).for_each(drop);
    ui.invoke_temporary_cue(0, false, false);
    assert!(!engine.snapshot(DeckId::A).playing);
    assert!(!ui.get_gig_rule_view().fulfilled);
    assert!(c
        .library
        .borrow()
        .gig_rule(b, id)
        .unwrap()
        .fulfilled_at_ms
        .is_none());
    ui.invoke_toggle_left();
    assert!(engine.snapshot(DeckId::A).playing);
    assert!(ui.get_gig_rule_view().fulfilled);
    let fulfilled = c
        .library
        .borrow()
        .gig_rule(b, id)
        .unwrap()
        .fulfilled_at_ms
        .unwrap();
    ui.invoke_toggle_left();
    ui.invoke_toggle_left();
    ui.invoke_save_gig_rule(id, "required".into(), "Played in the opening set".into());
    assert_eq!(
        c.library.borrow().gig_rule(b, id).unwrap().fulfilled_at_ms,
        Some(fulfilled)
    );
    choose_gig(&ui, "Gig A");
    assert_eq!(ui.get_gig_rule_view().status, "avoid");
    assert_eq!(ui.get_gig_rule_view().reason, "Reserved for the other room");
    c.close_library().unwrap();
    let store = crate::library_store::Worker::start(db).unwrap();
    let crate::library_store::Event::Loaded(Ok(saved)) = store.events.recv().unwrap() else {
        panic!("expected saved gig rules");
    };
    assert_eq!(saved.library.active_gig(), Some(a));
    assert_eq!(
        saved.library.gig_rule(a, id).unwrap().status,
        GigStatus::Avoid
    );
    assert_eq!(
        saved.library.gig_rule(b, id).unwrap().fulfilled_at_ms,
        Some(fulfilled)
    );
    assert_eq!(
        saved.library.gig_rule(b, id).unwrap().reason,
        "Played in the opening set"
    );
    store.close_delta(None).unwrap();
}
