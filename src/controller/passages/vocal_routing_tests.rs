fn wait_for_vocal_result(c: &AudioController, view: &PassageWindow, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        c.poll_passages();
        let status = view.get_vocal_status();
        if status.contains(message) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {message}, got {status}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn scan_vocal_selection(c: &AudioController, view: &PassageWindow, message: &str) {
    // Slint delivers the timing fields' changed callbacks on the UI tick between
    // editing the field and clicking Scan. These callbacks invalidate any older result.
    slint::platform::update_timers_and_animations();
    view.invoke_scan_vocals();
    wait_for_vocal_result(c, view, message);
}

#[test]
fn r08_real_stem_overlap_and_headphone_route() {
    use crate::controller::passages::loudness_qualification::{stem_key, write_stem_frames};
    use crate::test_support::{wav_file, wav_samples};
    use slint::Model;

    const CHILD: &str = "DRDJ_R08_CACHE_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let cache = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "controller::passages::vocals::acceptance::r08_real_stem_overlap_and_headphone_route", "--nocapture"])
            .env(CHILD, "1")
            .env("DRDJ_STEM_CACHE", cache.path())
            .output().unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            result.status.success() && stdout.contains("1 passed") && stderr.contains("R08 stereo"),
            "{stdout}{stderr}"
        );
        eprint!("{stderr}");
        return;
    }
    let dir = PathBuf::from(std::env::var_os("DRDJ_STEM_CACHE").unwrap());
    // Mark the first 200 ms of the expected audition with distinct L/R pulses.
    // Starting from zero, or a different point within the overlap, fails the PFL check.
    let make_source = |side: usize, salt: i16| {
        let mut pcm = vec![0_i16; 80_000 * 2];
        let begin = 48_000 + side * 800;
        for frame in begin..begin + 800 {
            pcm[2 * frame + side] = 8_000;
        }
        pcm[0] = salt;
        wav_samples(&pcm, 8_000, 2)
    };
    let a = make_source(0, 0);
    let key_a = stem_key(a.path());
    let (b, key_b) = (0..128)
        .find_map(|salt| {
            let b = make_source(1, salt);
            let key = stem_key(b.path());
            (key[0] % 2 != key_a[0] % 2).then_some((b, key))
        })
        .unwrap();
    let write = |key, first, last| {
        write_stem_frames(&dir, key, 441_000, |i| {
            let vocal = if (first..last).contains(&(i / 44_100)) {
                6500
            } else {
                0
            };
            [vocal, -vocal, 1000, 1000, 0, 0, 0, 0]
        });
    };
    write(key_a, 4, 8);
    write(key_b, 6, 10);

    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let (mixer, mut output) = rodio::mixer::mixer(4, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48_000));
    let controller = AudioController::wire(
        &ui,
        Rc::clone(&engine),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    let paths = [
        a.path().canonicalize().unwrap(),
        b.path().canonicalize().unwrap(),
    ];
    controller.library.borrow_mut().insert(paths.clone());
    controller.open_passages().unwrap();
    let view = controller
        .passages
        .borrow()
        .view
        .as_ref()
        .unwrap()
        .clone_strong();
    for (side, path) in paths.iter().enumerate() {
        let id = controller.library.borrow().track_id_for_path(path).unwrap();
        controller.library.borrow_mut().select(id);
        ui.set_selected_track(id);
        view.invoke_pin(side == 1);
    }
    view.set_length("10".into());
    scan_vocal_selection(&controller, &view, "Vocal overlaps on");
    assert_eq!(view.get_overlaps().row_count(), 1);
    assert_eq!(
        view.get_overlaps().row_data(0).unwrap().as_str(),
        "6.000 – 8.000 s"
    );

    let master = wav_file(4096, 48_000);
    let (dry_mixer, mut dry_output) = rodio::mixer::mixer(4, 48_000);
    let dry_engine = Rc::new(DualDeckEngine::from_mixer(dry_mixer, 4, 48_000));
    let dry_ui = AppWindow::new().unwrap();
    let _dry_controller = AudioController::wire(
        &dry_ui,
        dry_engine.clone(),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    for live in [&engine, &dry_engine] {
        live.load_a(master.path()).unwrap();
        live.play(DeckId::A);
    }
    let frame = |output: &mut rodio::mixer::MixerSource| -> [f32; 4] {
        std::array::from_fn(|_| output.next().unwrap())
    };
    controller.poll_passages();
    assert_eq!(
        view.get_overlap_index(),
        0,
        "the scan must select the first overlap"
    );
    assert!(
        view.get_vocal_route(),
        "the separate headphone pair must be available"
    );
    view.invoke_hear_overlap();
    assert!(
        controller.passages.borrow().vocals.owner.is_some(),
        "the selected range must reserve the preview"
    );
    wait_for_vocal_result(&controller, &view, "Playing selected overlap");
    let mut peaks = [0.0_f32; 2];
    let mut master_peak = 0.0_f32;
    for i in 0..9600 {
        let actual = frame(&mut output);
        let reference = frame(&mut dry_output);
        assert_eq!(actual[..2], reference[..2]);
        assert_eq!(reference[2..], [0., 0.]);
        master_peak = master_peak.max(actual[0].abs());
        if let Some(side) = match i {
            960..3840 => Some(0),
            5760..8640 => Some(1),
            _ => None,
        } {
            peaks[side] = peaks[side].max(actual[2 + side].abs());
            assert!(actual[3 - side].abs() < 0.001);
        }
    }
    assert!(master_peak > 0.01);
    assert!(peaks.iter().all(|&peak| peak > 0.02));
    assert!(engine.snapshot(DeckId::A).playing);
    view.invoke_stop_overlap();
    view.set_offset_b("4".into());
    scan_vocal_selection(&controller, &view, "No overlapping vocals");
    assert_eq!(view.get_overlaps().row_count(), 0);

    write(key_a, 0, 0);
    write(key_b, 0, 0);
    view.set_offset_b("0".into());
    scan_vocal_selection(&controller, &view, "No overlapping vocals");
    assert_eq!(view.get_overlaps().row_count(), 0);
    std::fs::remove_file(dir.join(format!("spleeter-v1-{}.stems", key_b[0] % 2))).unwrap();
    scan_vocal_selection(&controller, &view, "prepared stem unavailable");
    assert_eq!(view.get_overlaps().row_count(), 0);
    eprintln!("R08 stereo: 6.000–8.000 s, PFL peaks {peaks:?}, master unchanged for 9600 frames; +4 s, instrumental pair and missing B distinguished");
    controller.close_passages();
}
