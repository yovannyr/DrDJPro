#[test]
fn reference_piano_confirmation_and_reset_preserve_analysis_energy_and_second_track() {
    i_slint_backend_testing::init_no_event_loop();
    let first = wav_file(140, 48_000);
    let second = wav_file(290, 24_000);
    let paths = [first.path(), second.path()].map(|p| p.canonicalize().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("piano.sqlite3");
    let make = |ui: &AppWindow| {
        let (mixer, _) = rodio::mixer::mixer(4, 48_000);
        AudioController::wire(
            ui,
            Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48_000)),
            Arc::new(|| None),
            Arc::new(|| None),
        )
    };
    let ui = AppWindow::new().unwrap();
    let c = make(&ui);
    c.restore_library(Ok(db.clone()), paths.to_vec());
    finish_storage(&c);
    finish_import(&c);
    finish_storage(&c);
    let ids = paths
        .each_ref()
        .map(|p| c.library.borrow().track_id_for_path(p).unwrap());
    let mut metadata = c
        .library
        .borrow()
        .track_by_id(ids[0])
        .unwrap()
        .metadata
        .clone();
    metadata.analysis=Some(serde_json::from_value(serde_json::json!({
        "file_path":paths[0].to_str().unwrap(),"bpm":120,"gain":1,"musical_key":"5A","waveform_overview":[],"is_corrupt":false
    })).unwrap());
    c.library.borrow_mut().set_metadata(&paths[0], metadata);
    c.library_changed();
    ui.invoke_library_tab_selected(4);
    finish_search(&c);
    for (i, key, energy) in [(0, "8A", "7"), (1, "2B", "3")] {
        ui.invoke_select_library(ids[i]);
        ui.invoke_open_cover_labels();
        if i == 0 {
            assert!(ui
                .get_cover_label_editor()
                .status
                .contains("Estimated key: 5A"));
        }
        ui.invoke_save_cover_labels(ids[i], key.into(), energy.into());
    }
    finish_storage(&c);
    c.close_library().unwrap();
    drop(c);
    let next = AppWindow::new().unwrap();
    let c = make(&next);
    c.restore_library(Ok(db.clone()), Vec::new());
    finish_storage(&c);
    next.invoke_library_tab_selected(4);
    finish_search(&c);
    next.invoke_select_library(ids[0]);
    next.invoke_open_cover_labels();
    let displayed = next.get_cover_label_editor();
    assert_eq!(displayed.key, "8A");
    assert!(
        displayed.status.contains("DJ-confirmed key: 8A")
            && displayed.status.contains("Estimated key: 5A")
    );
    next.invoke_reset_confirmed_key(ids[0]);
    assert_eq!(next.get_cover_label_editor().key, "5A");
    let row = next
        .get_library_rows()
        .iter()
        .find(|r| r.id == ids[0])
        .unwrap();
    assert!(row.cover_key.is_empty());
    assert_eq!(row.cover_energy, "7");
    assert!(next
        .get_cover_label_editor()
        .status
        .starts_with("Estimated key: 5A"));
    next.invoke_cancel_cover_labels();
    finish_storage(&c);
    c.close_library().unwrap();
    let worker = crate::library_store::Worker::start(db).unwrap();
    let crate::library_store::Event::Loaded(Ok(saved)) = worker.events.recv().unwrap() else {
        panic!("catalog not loaded")
    };
    let first = &saved.library.track_by_id(ids[0]).unwrap().metadata;
    assert_eq!(first.music_labels.key(), None);
    assert_eq!(first.music_labels.energy(), Some(7));
    assert_eq!(first.analysis.as_ref().unwrap().musical_key, "5A");
    let other = saved
        .library
        .track_by_id(ids[1])
        .unwrap()
        .metadata
        .music_labels;
    assert_eq!(other.key().unwrap().to_string(), "2B");
    assert_eq!(other.energy(), Some(3));
    worker.close_delta(None).unwrap();
}

#[test]
fn reference_piano_cancels_loading_stale_selection_and_route_loss_in_both_layouts() {
    i_slint_backend_testing::init_no_event_loop();
    let first = wav_file(123, 48000);
    let second = wav_file(212, 24000);
    let paths = [first.path(), second.path()].map(|p| p.canonicalize().unwrap());
    for (layout, channels) in [
        (crate::LayoutMode::Classic, 2),
        (crate::LayoutMode::Modern, 4),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let ui = AppWindow::new().unwrap();
        let (mixer, _output) = rodio::mixer::mixer(channels, 48000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mixer, channels, 48000));
        let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
        c.restore_library(Ok(dir.path().join("library.sqlite3")), paths.to_vec());
        finish_storage(&c);
        finish_import(&c);
        finish_storage(&c);
        let ids = paths
            .each_ref()
            .map(|p| c.library.borrow().track_id_for_path(p).unwrap());
        ui.set_view_mode(layout);
        ui.set_modern_library_open(true);
        ui.invoke_library_tab_selected(4);
        finish_search(&c);
        ui.invoke_select_library(ids[0]);
        ui.invoke_open_cover_labels();
        assert_eq!(ui.get_reference_route(), channels == 4);
        let play = || ui.invoke_audition_reference(ids[0], "8A".into(), "0.25".into(), true);
        play();
        if channels == 2 {
            assert!(!ui.get_reference_active() && c.preview_job.borrow().is_none());
        } else {
            assert!(ui.get_reference_active());
            ui.invoke_select_library(ids[1]);
            assert!(!ui.get_reference_active() && !ui.get_cover_label_editor().open);
            ui.invoke_select_library(ids[0]);
            ui.invoke_open_cover_labels();
            let drain = || {
                let until = Instant::now() + Duration::from_secs(10);
                while c.preview_job.borrow().is_some() {
                    assert!(Instant::now() < until, "reference decoder did not finish");
                    c.tick();
                    std::thread::yield_now();
                }
            };
            drain();
            assert!(!ui.get_reference_active());
            ui.invoke_audition_reference(ids[0], "8A".into(), "99".into(), true);
            assert!(!ui.get_reference_active() && ui.get_reference_status().contains("outside"));
            play();
            drain();
            assert!(
                ui.get_reference_active()
                    && ui.get_reference_status().contains("Looping from 0.25")
            );
            ui.invoke_audition_reference(ids[1], "2B".into(), "0".into(), false);
            assert!(ui.get_reference_active());
            ui.invoke_stop_reference();
            assert!(!ui.get_reference_active());
            play();
            assert!(ui.get_reference_active());
            ui.set_output_pair_index(1);
            ui.invoke_output_pair_changed();
            assert!(!ui.get_reference_active() && !ui.get_reference_route());
            assert!(c.library.borrow().catalog().history.is_empty());
        }
        ui.invoke_cancel_cover_labels();
        c.close_library().unwrap();
    }
}
