fn test_wave(flatten: bool) -> crate::test_support::TempFile {
    let samples = (0..48_000 * 5)
        .map(|i| {
            let phase = (i % 48) as f64 * std::f64::consts::TAU / 48.;
            let tone = if flatten && (48_000..144_000).contains(&i) {
                (1.6 * phase.sin()).clamp(-0.98, 0.98)
            } else {
                0.98 * phase.sin()
            };
            (tone * f64::from(i16::MAX)).round() as i16
        })
        .collect::<Vec<_>>();
    crate::test_support::wav_samples(&samples, 48_000, 1)
}

fn finish_check(c: &AudioController, ui: &AppWindow) {
    let end = Instant::now() + Duration::from_secs(15);
    while ui.get_clip_check_busy() {
        c.tick();
        assert!(
            Instant::now() < end,
            "clip diagnosis timed out: {}",
            ui.get_clip_check_status()
        );
        std::thread::yield_now();
    }
}

#[test]
fn cached_clip_comparison_is_silent_when_source_is_replaced_under_same_library_id() {
    i_slint_backend_testing::init_no_event_loop();
    let clipped = test_wave(true);
    let clean = test_wave(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selected.wav");
    std::fs::copy(clipped.path(), &path).unwrap();
    let path = path.canonicalize().unwrap();
    let modified = path.metadata().unwrap().modified().unwrap();

    let ui = AppWindow::new().unwrap();
    let (mixer, mut output) = rodio::mixer::mixer(4, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48_000));
    let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
    c.restore_library(Ok(dir.path().join("clip.sqlite3")), vec![path.clone()]);
    finish_storage(&c);
    finish_import(&c);
    finish_storage(&c);
    let id = c.library.borrow().track_id_for_path(&path).unwrap();
    ui.invoke_library_tab_selected(4);
    finish_search(&c);
    ui.invoke_select_library(id);
    ui.invoke_open_clip_check();
    finish_check(&c, &ui);
    ui.invoke_audition_clip_check(false);
    finish_check(&c, &ui);
    assert!(ui.get_clip_check_status().contains("Playing original"));
    let audible = output
        .by_ref()
        .take(4096)
        .enumerate()
        .any(|(sample, value)| sample % 4 >= 2 && value.abs() > 0.001);
    assert!(audible, "initial audition must reach the headphone output");

    // Both files have identical lengths and the replacement gets the original mtime.
    std::fs::rename(&path, dir.path().join("previous.wav")).unwrap();
    std::fs::copy(clean.path(), &path).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(ui.get_selected_track(), id);
    ui.invoke_audition_clip_check(true);
    finish_check(&c, &ui);
    assert!(
        ui.get_clip_check_status().contains("changed"),
        "{}",
        ui.get_clip_check_status()
    );
    // Drain a pending interleaved frame and verify that the cancelled voice stays silent.
    output.by_ref().take(8).for_each(drop);
    assert!(output.by_ref().take(4096).all(|v| v.abs() < 1e-7));
    ui.invoke_close_clip_check();
    c.close_library().unwrap();
}

#[test]
fn clip_check_compares_on_pfl_exports_only_on_confirmation_and_cancels_stale_picker() {
    use sha2::{Digest, Sha256};
    i_slint_backend_testing::init_no_event_loop();
    let flat = test_wave(true);
    let clean = test_wave(false);
    let paths = [flat.path(), clean.path()].map(|p| p.canonicalize().unwrap());
    let checksum = || Sha256::digest(std::fs::read(&paths[0]).unwrap());
    let before = checksum();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("exports");
    std::fs::create_dir(&dest).unwrap();
    let outputs = || {
        std::fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>()
    };
    let ui = AppWindow::new().unwrap();
    let (mixer, mut out) = rodio::mixer::mixer(4, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48_000));
    let c = AudioController::wire(&ui, engine.clone(), Arc::new(|| None), Arc::new(|| None));
    c.restore_library(Ok(dir.path().join("clipcheck.sqlite3")), paths.to_vec());
    finish_storage(&c);
    finish_import(&c);
    finish_storage(&c);
    for deck in [DeckId::A, DeckId::B] {
        engine.install(deck, PreparedTrack::open(clean.path()).unwrap());
        engine.play(deck);
    }
    let ids = paths
        .each_ref()
        .map(|p| c.library.borrow().track_id_for_path(p).unwrap());
    ui.set_view_mode(crate::LayoutMode::Modern);
    ui.set_modern_library_open(true);
    ui.invoke_library_tab_selected(4);
    finish_search(&c);
    let open = |id| {
        ui.invoke_select_library(id);
        ui.invoke_open_clip_check();
        assert!(ui.get_clip_check_open() && ui.get_clip_check_visible());
        assert_eq!(ui.get_clip_check_track(), id);
        finish_check(&c, &ui);
    };
    open(ids[0]);
    assert!(
        ui.get_clip_check_markers().row_count() > 0,
        "{}",
        ui.get_clip_check_status()
    );
    assert_eq!(ui.get_clip_check_index(), 0);
    assert!(ui.get_clip_check_route());
    ui.invoke_select_clip_marker(0);
    ui.invoke_audition_clip_check(false);
    finish_check(&c, &ui);
    assert!(
        ui.get_clip_check_status().contains("Playing original"),
        "{}",
        ui.get_clip_check_status()
    );
    out.by_ref().take(768).for_each(drop);
    ui.invoke_audition_clip_check(true);
    finish_check(&c, &ui);
    assert!(!ui.get_clip_check_busy() && ui.get_clip_check_status().contains("Playing processed"));
    out.by_ref().take(768).for_each(drop);
    ui.invoke_stop_clip_check();
    assert!(ui.get_clip_check_status().contains("stopped"));
    assert!(outputs().is_empty());
    assert_eq!(checksum(), before);

    let (started, entered) = mpsc::sync_channel(1);
    let (resume, blocked) = mpsc::sync_channel(1);
    let blocked = std::sync::Mutex::new(blocked);
    let stale_path = dest.clone();
    c.set_library_folder_picker(Arc::new(move || {
        started.send(()).unwrap();
        blocked.lock().unwrap().recv().unwrap();
        Some(stale_path.clone())
    }));
    ui.invoke_export_clip_check();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    ui.invoke_select_library(ids[1]);
    assert!(!ui.get_clip_check_open());
    resume.send(()).unwrap();
    open(ids[1]);
    assert_eq!(ui.get_clip_check_markers().row_count(), 0);
    assert!(ui.get_clip_check_status().contains("No suspected"));
    assert!(outputs().is_empty());
    open(ids[0]);
    let out_dir = dest.clone();
    c.set_library_folder_picker(Arc::new(move || Some(out_dir.clone())));
    ui.invoke_export_clip_check();
    finish_check(&c, &ui);
    assert!(
        ui.get_clip_check_status().contains("Saved separate WAV"),
        "{}",
        ui.get_clip_check_status()
    );
    let files = outputs();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].extension().unwrap(), "wav");
    assert_eq!(checksum(), before);
    assert!(engine.snapshot(DeckId::A).playing && engine.snapshot(DeckId::B).playing);
    ui.set_output_pair_index(1);
    ui.invoke_output_pair_changed();
    assert!(!ui.get_clip_check_route());
    ui.invoke_audition_clip_check(true);
    assert!(ui.get_clip_check_status().contains("separate headphone"));
    ui.invoke_close_clip_check();
    assert!(!ui.get_clip_check_open());
    c.close_library().unwrap();
}
