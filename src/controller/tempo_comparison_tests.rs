#[test]
fn prepare_tempo_controls_compare_apply_and_cancel_on_both_layouts() {
    use crate::beatgrid::BeatGrid;
    use slint::Model;
    i_slint_backend_testing::init_no_event_loop();
    // Exercise the actual stereo visual fallback and an independent 3/4 headphone pair.
    for (layout, channels) in [
        (crate::LayoutMode::Classic, 2),
        (crate::LayoutMode::Modern, 4),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite3");
        let file = wav_file(4096, 48_000 * 5);
        let other = wav_file(2048, 48_000 * 3);
        let path = std::fs::canonicalize(file.path()).unwrap();
        let other_path = std::fs::canonicalize(other.path()).unwrap();
        let (mixer, mut output) = rodio::mixer::mixer(channels, 48_000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mixer, channels, 48_000));
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1280., 860.));
        let c = AudioController::wire(
            &ui,
            Rc::clone(&engine),
            Arc::new(|| None),
            Arc::new(|| None),
        );
        ui.show().unwrap();
        c.restore_library(Ok(db.clone()), vec![path.clone(), other_path.clone()]);
        finish_storage(&c);
        finish_import(&c);
        finish_storage(&c);
        let id = c.library.borrow().track_id_for_path(&path).unwrap();
        let other_id = c.library.borrow().track_id_for_path(&other_path).unwrap();
        let anchor = Duration::from_secs(1);
        let slow = BeatGrid::new(85., anchor).unwrap();
        let fast = BeatGrid::new(170., anchor).unwrap();
        c.library.borrow_mut().set_grid(&path, Some(slow)).unwrap();
        c.library_changed();
        finish_storage(&c);

        ui.set_view_mode(layout);
        ui.set_modern_library_open(true);
        ui.invoke_library_tab_selected(4);
        finish_search(&c);
        assert_eq!(ui.get_library_tab(), 4);
        ui.invoke_select_library(id);
        assert_eq!(ui.get_selected_track(), id, "Prepare row must be selected");
        c.tick();
        engine.load_a(&path).unwrap();
        engine.set_grid(DeckId::A, Some(slow)).unwrap();
        engine.set_speed(DeckId::A, 1.04).unwrap();
        engine.play(DeckId::A);
        engine.apply_sync_factor(DeckId::A, 1.005).unwrap();
        output
            .by_ref()
            .take(usize::from(channels) * 2048)
            .for_each(drop);
        let master = engine.snapshot(DeckId::A);
        assert!(master.playing && master.position > Duration::ZERO);
        let press = |label: &str| control(&ui, label).invoke_accessible_default_action();
        press("Compare half and double tempo");
        let view = ui.get_tempo_comparison();
        assert!(
            view.open && !view.selected_fast && view.can_apply,
            "Prepare compare state: open={}, fast={}, writable={}, notice={}, status={}",
            view.open,
            view.selected_fast,
            view.can_apply,
            view.status,
            ui.get_status()
        );
        assert_eq!(view.track_id, id);
        assert_eq!(view.slow_label.as_str(), "85.00 BPM");
        assert_eq!(view.fast_label.as_str(), "170.00 BPM");
        assert_eq!(view.anchor_label.as_str(), "Downbeat 1.000 s");
        assert_eq!(
            view.status.contains("Visual comparison only"),
            channels == 2
        );
        let slow_lines = view.beats.row_count();
        assert!(slow_lines > 0);
        press("Faster tempo interpretation");
        let selected = ui.get_tempo_comparison();
        assert!(selected.selected_fast && selected.beats.row_count() > slow_lines);
        assert_eq!(selected.anchor_label, view.anchor_label);
        if channels == 4 {
            let until = Instant::now() + Duration::from_secs(15);
            while c.preview_job.borrow().is_some() {
                assert!(Instant::now() < until, "headphone excerpt decode timed out");
                c.tick();
                std::thread::yield_now();
            }
            assert!(
                ui.get_tempo_comparison().status.is_empty(),
                "{}",
                ui.get_tempo_comparison().status
            );
        }
        press("Slower tempo interpretation");
        press("Faster tempo interpretation");
        assert_eq!(c.library.borrow().grid(&path), Some(slow));
        press("Apply tempo interpretation");
        assert!(!ui.get_tempo_comparison().open);
        assert_eq!(c.library.borrow().grid(&path), Some(fast));
        // The deterministic output iterator has not advanced since the baseline.
        let after = engine.snapshot(DeckId::A);
        assert!(after.playing);
        assert_eq!(after.path, master.path);
        assert_eq!(after.position, master.position);
        assert_eq!(after.requested_speed, master.requested_speed);
        assert_eq!(after.effective_speed, master.effective_speed);
        assert_eq!(after.sync_factor, master.sync_factor);
        assert_eq!(after.grid, master.grid);

        press("Compare half and double tempo");
        assert!(ui.get_tempo_comparison().selected_fast);
        press("Slower tempo interpretation");
        press("Cancel tempo comparison");
        assert!(!ui.get_tempo_comparison().open);
        press("Compare half and double tempo");
        press("Slower tempo interpretation");
        finish_search(&c);
        ui.invoke_select_library(other_id);
        assert_eq!(
            ui.get_selected_track(),
            other_id,
            "stale draft must target a different row"
        );
        // Cover delayed view updates: an Apply arriving before the next tick must still be safe.
        ui.invoke_apply_tempo_comparison();
        c.tick();
        assert!(!ui.get_tempo_comparison().open);
        assert_eq!(c.library.borrow().grid(&path), Some(fast));
        assert_eq!(c.library.borrow().grid(&other_path), None);
        finish_storage(&c);
        c.close_library().unwrap();
        ui.hide().unwrap();
        drop(c);
        let reopened = AppWindow::new().unwrap();
        let restored = AudioController::wire(
            &reopened,
            engine.clone(),
            Arc::new(|| None),
            Arc::new(|| None),
        );
        restored.restore_library(Ok(db), vec![]);
        finish_storage(&restored);
        assert_eq!(restored.library.borrow().grid(&path), Some(fast));
        assert_eq!(restored.library.borrow().grid(&other_path), None);
        restored.close_library().unwrap();
    }
}
