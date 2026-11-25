fn until(c: &AudioController, finished: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        c.tick();
        if finished() {
            break;
        }
        assert!(Instant::now() < end, "sample browser did not settle");
        std::thread::yield_now();
    }
}

#[test]
fn twenty_samples_preview_then_assign_only_one_slot_and_reopen_bank() {
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("kit");
    std::fs::create_dir(&folder).unwrap();
    for i in 0..20 {
        let file = wav_file(800 + 200 * i, 2400);
        std::fs::copy(file.path(), folder.join(format!("sample-{i:02}.wav"))).unwrap();
    }
    std::fs::create_dir(folder.join("folder.wav")).unwrap();
    std::fs::write(folder.join("not-audio.txt"), b"excluded").unwrap();
    let original_sample = wav_file(2000, 48_000);
    let source = original_sample.path().to_path_buf();

    for channels in [4, 2] {
        let db = dir.path().join(format!("sampler-{channels}.sqlite3"));
        let ui = AppWindow::new().unwrap();
        let (mix, mut output) = rodio::mixer::mixer(channels, 48_000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mix, channels, 48_000));
        let picked = source.clone();
        let c = AudioController::wire(
            &ui,
            Rc::clone(&engine),
            Arc::new(move || Some(picked.clone())),
            Arc::new(|| None),
        );
        c.restore_library(Ok(db.clone()), vec![]);
        finish_storage(&c);
        let pick_dir = folder.clone();
        c.set_library_folder_picker(Arc::new(move || Some(pick_dir.clone())));
        let saved = || {
            until(&c, || {
                let s = c.sampler.borrow();
                s.job.is_none() && !s.saving && s.saved == s.preferences
            });
        };
        for slot in 0..4 {
            ui.invoke_sampler_action(slot, 2, false);
            saved();
        }
        let original = c.sampler.borrow().preferences.clone();
        assert!(original.paths.iter().all(Option::is_some));
        for deck in [DeckId::A, DeckId::B] {
            engine.install(deck, PreparedTrack::open(&source).unwrap());
            engine.play(deck);
        }
        output.by_ref().take(1024).for_each(drop);
        let open = || {
            ui.set_sampler_open(true);
            ui.invoke_open_sample_browser(2);
            assert!(ui.get_samp_preview_open());
            assert_eq!(ui.get_samp_preview_slot(), 2);
            assert_eq!(ui.get_samp_preview_route(), channels == 4);
            ui.invoke_choose_sample_folder();
            until(&c, || !ui.get_samp_preview_busy());
            let names = ui.get_samp_preview_files();
            assert_eq!(names.row_count(), 20);
            assert_eq!(names.row_data(0).unwrap(), "sample-00.wav");
        };
        open();
        for i in 0..20 {
            ui.invoke_select_sample_preview(i);
            assert_eq!(ui.get_samp_preview_index(), i);
            until(&c, || c.preview_job.borrow().is_none());
            if channels == 4 {
                assert!(
                    ui.get_samp_preview_status().contains("Headphone preview"),
                    "{}",
                    ui.get_samp_preview_status()
                );
                assert!(
                    ui.get_samp_preview_bytes() > 0
                        && ui.get_samp_preview_bytes() as usize
                            <= crate::playback::MAX_SAMPLE_PREVIEW_BYTES
                );
            } else {
                assert!(ui
                    .get_samp_preview_status()
                    .contains("No separate headphone"));
                assert_eq!(ui.get_samp_preview_bytes(), 0);
            }
            output.by_ref().take(64).for_each(drop);
        }
        for i in (0..20).rev() {
            ui.invoke_select_sample_preview(i);
        }
        ui.invoke_close_sample_browser();
        until(&c, || c.preview_job.borrow().is_none());
        assert!(!ui.get_samp_preview_open());
        assert_eq!(ui.get_samp_preview_bytes(), 0);
        assert_eq!(c.sampler.borrow().preferences, original);
        assert_eq!(c.sampler.borrow().saved, original);
        for deck in [DeckId::A, DeckId::B] {
            assert!(engine.snapshot(deck).playing);
        }

        open();
        ui.invoke_select_sample_preview(12);
        ui.invoke_assign_sample_preview();
        assert!(!ui.get_samp_preview_open());
        saved();
        let mut expected = original;
        expected.paths[2] = Some(folder.join("sample-12.wav").canonicalize().unwrap());
        assert_eq!(c.sampler.borrow().preferences, expected);
        c.close_library().unwrap();
        drop(c);
        let worker = crate::library_store::Worker::start(db).unwrap();
        let crate::library_store::Event::Loaded(Ok(loaded)) = worker.events.recv().unwrap() else {
            panic!("library reopened")
        };
        assert_eq!(loaded.sampler_preferences.unwrap(), expected);
        worker.close_delta(None).unwrap();
    }
}

#[test]
fn abandoned_native_picker_cannot_populate_reopened_sample_browser() {
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("old");
    let current = dir.path().join("new");
    for (path, name) in [(&blocked, "old"), (&current, "current")] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join(format!("{name}.wav")), []).unwrap();
    }
    let ui = AppWindow::new().unwrap();
    let (mix, _output) = rodio::mixer::mixer(4, 48_000);
    let c = AudioController::wire(
        &ui,
        Rc::new(DualDeckEngine::from_mixer(mix, 4, 48_000)),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    c.restore_library(Ok(dir.path().join("bank.sqlite3")), vec![]);
    finish_storage(&c);
    let (started, enter) = mpsc::sync_channel(1);
    let (release, blocked_until) = mpsc::sync_channel(1);
    let blocked_until = std::sync::Mutex::new(blocked_until);
    c.set_library_folder_picker(Arc::new(move || {
        started.send(()).unwrap();
        blocked_until.lock().unwrap().recv().unwrap();
        Some(blocked.clone())
    }));
    ui.set_sampler_open(true);
    ui.invoke_open_sample_browser(0);
    ui.invoke_choose_sample_folder();
    enter.recv_timeout(Duration::from_secs(5)).unwrap();
    ui.invoke_close_sample_browser();
    c.set_library_folder_picker(Arc::new(move || Some(current.clone())));
    ui.invoke_open_sample_browser(1);
    ui.invoke_choose_sample_folder();
    assert!(ui.get_samp_preview_busy());
    release.send(()).unwrap();
    until(&c, || !ui.get_samp_preview_busy());
    assert_eq!(ui.get_samp_preview_slot(), 1);
    let files = ui.get_samp_preview_files();
    assert_eq!(files.row_count(), 1);
    assert_eq!(files.row_data(0).unwrap(), "current.wav");
    ui.invoke_close_sample_browser();
    c.close_library().unwrap();
}
