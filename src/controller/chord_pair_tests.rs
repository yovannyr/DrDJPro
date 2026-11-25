fn wait_chords(c: &AudioController, ui: &AppWindow) {
    let until = Instant::now() + Duration::from_secs(20);
    while ui.get_chord_pair_busy() {
        c.tick();
        assert!(Instant::now() < until, "{}", ui.get_chord_pair_status());
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn pair_settings() -> crate::ChordPairSettings {
    crate::ChordPairSettings {
        start_a: "0".into(),
        start_b: "0".into(),
        length: "2".into(),
        offset_b: "0.5".into(),
        tempo_a: "1".into(),
        tempo_b: "1".into(),
        pitch_a: "0".into(),
        pitch_b: "2".into(),
    }
}

#[test]
fn pinned_chord_tracks_align_confirm_audition_restart_and_reject_replaced_source() {
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("chords.sqlite3");
    let triad = |notes: [i32; 3]| {
        let rates = notes.map(|n| 440. * 2f64.powf(f64::from(n - 69) / 12.));
        let samples = (0..3 * 48_000)
            .map(|i| {
                let t = f64::from(i) * std::f64::consts::TAU / 48_000.;
                (rates.iter().map(|f| (f * t).sin()).sum::<f64>() * 4000.).round() as i16
            })
            .collect::<Vec<_>>();
        crate::test_support::wav_samples(&samples, 48_000, 1)
    };
    let a = triad([50, 53, 57]);
    let b = triad([57, 61, 64]);
    let paths = [a.path(), b.path()].map(|p| p.canonicalize().unwrap());
    for session in 0..2 {
        let ui = AppWindow::new().unwrap();
        let (mixer, mut output) = rodio::mixer::mixer(4, 48_000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48_000));
        let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
        c.restore_library(
            Ok(db.clone()),
            if session == 0 {
                paths.to_vec()
            } else {
                Vec::new()
            },
        );
        finish_storage(&c);
        if session == 0 {
            finish_import(&c);
            finish_storage(&c);
        }
        let ids = paths
            .each_ref()
            .map(|p| c.library.borrow().track_id_for_path(p).unwrap());
        ui.set_view_mode(if session == 0 {
            crate::LayoutMode::Classic
        } else {
            crate::LayoutMode::Modern
        });
        ui.set_modern_library_open(true);
        ui.invoke_library_tab_selected(4);
        finish_search(&c);
        ui.invoke_open_chord_pair();
        for (side, id) in ids.into_iter().enumerate() {
            ui.invoke_select_library(id);
            ui.invoke_use_chord_track(side as i32);
        }
        ui.invoke_select_library(ids[0]);
        assert!(
            ui.get_chord_pair_visible()
                && !ui.get_chord_pair_a().is_empty()
                && !ui.get_chord_pair_b().is_empty()
        );
        ui.invoke_compare_chord_pair(pair_settings());
        wait_chords(&c, &ui);
        let rows = ui.get_chord_pair_rows();
        assert!(rows.row_count() >= 2, "{}", ui.get_chord_pair_status());
        assert!(rows.row_data(0).unwrap().b.contains("No audio"));
        ui.invoke_select_chord_row(0);
        ui.invoke_confirm_chord_event(1, "Am".into());
        assert!(ui.get_chord_pair_status().contains("no source event"));
        let entry = rows
            .iter()
            .position(|row| !row.b.contains("No audio"))
            .unwrap();
        assert!(rows.row_data(entry).unwrap().time.starts_with("0.500"));
        ui.invoke_select_chord_row(entry as i32);
        if session == 0 {
            ui.invoke_confirm_chord_event(1, "Dm".into());
            wait_chords(&c, &ui);
            assert!(
                ui.get_chord_pair_status().contains("confirmed and saved"),
                "{}",
                ui.get_chord_pair_status()
            );
        }
        let display = ui.get_chord_pair_rows().row_data(entry).unwrap().b;
        assert!(
            display.contains("D:min → E:min heard") && display.contains("confirmed"),
            "{display}"
        );
        if session == 0 {
            ui.invoke_confirm_chord_event(1, "".into());
            wait_chords(&c, &ui);
            assert!(ui.get_chord_pair_status().contains("confirmed and saved"));
            ui.invoke_audition_chord_pair();
            wait_chords(&c, &ui);
            assert!(
                ui.get_chord_pair_status().contains("Playing aligned"),
                "{}",
                ui.get_chord_pair_status()
            );
            let frames = output.by_ref().take(4096).collect::<Vec<_>>();
            assert!(frames
                .as_chunks::<4>().0.iter()
                .any(|f| f[2].abs() > 0.0001 || f[3].abs() > 0.0001));
            assert!(frames.as_chunks::<4>().0.iter().all(|f| f[0] == 0. && f[1] == 0.));
            ui.invoke_stop_chord_pair();
            assert!(output.by_ref().take(1024).all(|s| s == 0.));
            // Pin the same source twice. A correction belongs to the source event on both lanes.
            ui.invoke_use_chord_track(1);
            let mut same = pair_settings();
            same.offset_b = "0".into();
            ui.invoke_compare_chord_pair(same);
            wait_chords(&c, &ui);
            ui.invoke_select_chord_row(0);
            ui.invoke_confirm_chord_event(0, "Unknown".into());
            wait_chords(&c, &ui);
            let row = ui.get_chord_pair_rows().row_data(0).unwrap();
            assert!([row.a, row.b]
                .iter()
                .all(|s| s.contains("Unknown") && s.contains("confirmed")));
        } else {
            let stamp = paths[1].metadata().unwrap().modified().unwrap();
            std::fs::rename(&paths[1], dir.path().join("previous.wav")).unwrap();
            std::fs::copy(&paths[0], &paths[1]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&paths[1])
                .unwrap()
                .set_modified(stamp)
                .unwrap();
            ui.invoke_audition_chord_pair();
            wait_chords(&c, &ui);
            assert!(
                ui.get_chord_pair_status().contains("changed")
                    && ui.get_chord_pair_rows().row_count() == 0,
                "{}",
                ui.get_chord_pair_status()
            );
            assert!(output.by_ref().take(1024).all(|s| s == 0.));
        }
        ui.invoke_library_tab_selected(0);
        assert!(!ui.get_chord_pair_open());
        c.close_library().unwrap();
    }
}
