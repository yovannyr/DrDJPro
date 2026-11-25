fn wait_local_keys(c: &AudioController, ui: &AppWindow) {
    let end = Instant::now() + Duration::from_secs(15);
    while ui.get_local_keys_busy() {
        c.tick();
        assert!(
            Instant::now() < end,
            "local keys timed out: {}",
            ui.get_local_keys_status()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn local_keys_use_verified_cache_project_live_grid_and_discard_replaced_source() {
    use crate::{
        beatgrid::BeatGrid,
        cancellation::Cancellation,
        local_harmony::{fingerprint, KeySection, LocalKeyProfile, ALGORITHM_VERSION},
        track_metadata::MusicLabels,
    };
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("keys.sqlite3");
    let initial = wav_file(15, 48_000 * 3);
    let replacement = wav_file(0, 48_000 * 3);
    let path = dir.path().join("source.wav");
    std::fs::copy(initial.path(), &path).unwrap();
    let path = path.canonicalize().unwrap();
    let mtime = path.metadata().unwrap().modified().unwrap();
    let make = |ui: &AppWindow| {
        let (mixer, _) = rodio::mixer::mixer(2, 48_000);
        AudioController::wire(
            ui,
            Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000)),
            Arc::new(|| None),
            Arc::new(|| None),
        )
    };
    let ui = AppWindow::new().unwrap();
    let c = make(&ui);
    c.restore_library(Ok(db.clone()), vec![path.clone()]);
    finish_storage(&c);
    finish_import(&c);
    finish_storage(&c);
    let id = c.library.borrow().track_id_for_path(&path).unwrap();
    let labels = MusicLabels::parse("11A", "8").unwrap();
    assert!(c.library.borrow_mut().set_music_labels(id, labels).unwrap());
    c.library_changed();
    finish_storage(&c);
    let profile = LocalKeyProfile {
        algorithm_version: ALGORITHM_VERSION,
        source_sha256: fingerprint(&path, &Cancellation::default()).unwrap(),
        duration_ms: 3000,
        segments: vec![
            KeySection {
                start_ms: 0,
                end_ms: 1500,
                key: Some("8B".into()),
                confidence: 90,
            },
            KeySection {
                start_ms: 1500,
                end_ms: 2500,
                key: Some("10B".into()),
                confidence: 84,
            },
            KeySection {
                start_ms: 2500,
                end_ms: 3000,
                key: None,
                confidence: 0,
            },
        ],
    };
    c.persistence
        .borrow()
        .as_ref()
        .unwrap()
        .local_key_cache()
        .save(id, &path, &profile)
        .unwrap();
    c.close_library().unwrap();
    drop(c);

    let ui = AppWindow::new().unwrap();
    ui.set_view_mode(crate::LayoutMode::Modern);
    ui.set_modern_library_open(true);
    let c = make(&ui);
    c.restore_library(Ok(db), Vec::new());
    finish_storage(&c);
    ui.invoke_library_tab_selected(4);
    finish_search(&c);
    ui.invoke_select_library(id);
    ui.invoke_open_local_keys();
    wait_local_keys(&c, &ui);
    assert!(ui.get_local_keys_visible());
    assert!(
        ui.get_local_keys_status().contains("Restored verified")
            && ui.get_local_keys_status().contains("No saved beatgrid")
    );
    assert_eq!(ui.get_local_keys_sections().row_count(), 3);
    assert!(ui.get_local_keys_entry().contains("C Major / 8B (90%)"));
    assert!(
        ui.get_local_keys_exit().contains("2.999 s")
            && ui.get_local_keys_exit().contains("Unknown")
    );
    let times = |entry: &str, exit: &str| {
        ui.invoke_select_local_key_times(entry.into(), exit.into());
        assert!(ui.get_local_keys_entry().is_empty());
        wait_local_keys(&c, &ui);
    };
    times("1.499", "2.5");
    assert!(
        ui.get_local_keys_entry().contains("8B") && ui.get_local_keys_exit().contains("Unknown")
    );
    times("1.5", "2.499");
    assert!(ui.get_local_keys_entry().contains("D Major / 10B (84%)"));
    let grid = BeatGrid::new(120., Duration::ZERO)
        .unwrap()
        .with_time_signature_change(2, 6, 8)
        .unwrap();
    c.library.borrow_mut().set_grid(&path, Some(grid)).unwrap();
    c.tick();
    assert!(ui.get_local_keys_entry().contains("bar 1 beat 4"));
    assert!(ui.get_local_keys_exit().contains("bar 2 beat 2"));
    let timeline = ui.get_local_keys_sections();
    assert!(timeline.row_data(1).unwrap().contains("bar 1 beat 4"));
    times("1e999", "-1");
    assert!(ui.get_local_keys_entry().contains("Enter non-negative"));
    times("1.5", "");

    // Replace the source under its original name and restore the timestamp and length.
    std::fs::rename(&path, dir.path().join("archived.wav")).unwrap();
    std::fs::copy(replacement.path(), &path).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    // Filesystems without a reliable native identity use the 30-second digest fallback.
    let deadline = Instant::now() + Duration::from_secs(45);
    while !ui.get_local_keys_status().contains("Local keys analyzed.") {
        c.tick();
        assert!(
            Instant::now() < deadline,
            "stale profile: {}",
            ui.get_local_keys_status()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ui.get_local_keys_entry().contains("Unknown"));
    assert_eq!(ui.get_local_keys_sections().row_count(), 1);
    assert_eq!(
        c.library
            .borrow()
            .track_by_id(id)
            .unwrap()
            .metadata
            .music_labels,
        labels
    );
    ui.invoke_library_tab_selected(0);
    assert!(!ui.get_local_keys_open() && ui.get_local_keys_sections().row_count() == 0);
    c.close_library().unwrap();
}

#[test]
fn bar_position_uses_the_effective_meter_denominator_anchor_and_warp() {
    use crate::beatgrid::BeatGrid;
    let grid = BeatGrid::new(120., Duration::from_secs(1))
        .unwrap()
        .with_time_signature(3, 4)
        .unwrap()
        .with_time_signature_change(2, 6, 8)
        .unwrap()
        .with_warp_marker(3, Duration::from_secs(3))
        .unwrap();
    assert_eq!(grid.bar_beat_at_ms(1000), (1, 1));
    assert_eq!(grid.bar_beat_at_ms(2999), (1, 3));
    assert_eq!(grid.bar_beat_at_ms(3000), (2, 1));
    assert_eq!(grid.bar_beat_at_ms(3250), (2, 2));
}
