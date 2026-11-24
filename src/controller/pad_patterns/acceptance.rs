use super::*;
use crate::sampler::phrase::Hit;

fn wait(c: &AudioController, ready: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        c.tick();
        if ready() {
            return;
        }
        assert!(Instant::now() < until, "pattern controller timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn press(ui: &AppWindow, c: &AudioController, pad: u8, at: Instant) {
    let command = crate::mc4000::decode(&[0x9f, 33 + pad, 127]).unwrap();
    c.with_midi_time(at, || crate::mc4000::apply(ui, command));
}

#[test]
fn m09_disconnected_pad_take_requires_confirmation_and_survives_reopen() {
    i_slint_backend_testing::init_no_event_loop();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("patterns.sqlite3");
    let sample = crate::test_support::wav_file(8192, 4800);
    let expected: Vec<Hit> = [0, 250_000, 375_000, 750_000]
        .into_iter()
        .enumerate()
        .map(|(pad, micros)| Hit {
            pad: pad as u8,
            micros,
        })
        .collect();
    for restart in [false, true] {
        let ui = AppWindow::new().unwrap();
        let (mixer, _sink) = rodio::mixer::mixer(2, 48_000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000));
        let path = sample.path().to_owned();
        let c = AudioController::wire(
            &ui,
            engine.clone(),
            Arc::new(move || Some(path.clone())),
            Arc::new(|| None),
        );
        c.restore_library(Ok(db.clone()), vec![]);
        wait(&c, || {
            c.storage_ready.get() && c.pad_patterns.borrow().book.is_some()
        });
        ui.invoke_open_pad_patterns();
        let view = c
            .pad_patterns
            .borrow()
            .view
            .as_ref()
            .unwrap()
            .clone_strong();
        if restart {
            let book = c.pad_patterns.borrow().book.clone().unwrap();
            assert_eq!(book.base.hits, expected);
            assert_eq!(book.fill.hits, expected[..2]);
            assert!(!view.get_draft() && !view.get_recording());
            c.close_library().unwrap();
            continue;
        }
        ui.set_mc4000_connected(true);
        for pad in 0..4 {
            ui.invoke_sampler_action(pad, 2, false);
            wait(&c, || {
                let s = c.sampler.borrow();
                s.job.is_none() && !s.saving && engine.sampler().ready(pad as usize)
            });
        }
        press(&ui, &c, 3, Instant::now());
        assert!(c.pad_patterns.borrow().take.is_none());
        // Timestamped hardware input is delivered in one batch. Use a past recording
        // origin so that all onsets precede the stop/disconnect without sleeping.
        let record = |fill, count: usize| {
            view.invoke_record_take(fill, 120);
            let start = Instant::now() - Duration::from_secs(1);
            c.pad_patterns.borrow_mut().take.as_mut().unwrap().start = start;
            press(&ui, &c, 0, start - Duration::from_millis(1));
            for hit in &expected[..count] {
                press(
                    &ui,
                    &c,
                    hit.pad,
                    start + Duration::from_micros(hit.micros.into()),
                );
            }
        };
        record(false, 4);
        view.invoke_stop_recording();
        press(&ui, &c, 1, Instant::now() + Duration::from_millis(1));
        assert_eq!(
            c.pad_patterns.borrow().take.as_ref().unwrap().pattern.hits,
            expected
        );
        let preview_epoch = engine.preview_epoch();
        assert!(!view.get_has_headphones());
        view.invoke_audition_take();
        assert!(view.get_status().contains("headphone"));
        assert_eq!(engine.preview_epoch(), preview_epoch);
        view.invoke_discard_take();
        assert_eq!(
            c.pad_patterns.borrow().book.as_ref().unwrap(),
            &Patterns::default()
        );

        let confirm = || {
            view.invoke_accept_take();
            view.invoke_accept_take();
            wait(&c, || c.pad_patterns.borrow().take.is_none());
            assert!(!view.get_can_accept());
        };
        record(false, 4);
        view.invoke_stop_recording();
        confirm();
        record(true, 2);
        // Same recovery and connection flags used by main's device discovery.
        c.mc4000_release_holds();
        ui.set_mc4000_connected(false);
        c.tick();
        assert!(!view.get_recording() && view.get_can_accept());
        assert!(c
            .pad_patterns
            .borrow()
            .book
            .as_ref()
            .unwrap()
            .fill
            .hits
            .is_empty());
        ui.set_mc4000_connected(true);
        c.tick();
        assert!(!view.get_recording() && view.get_can_accept());
        press(&ui, &c, 2, Instant::now() + Duration::from_millis(1));
        assert_eq!(
            c.pad_patterns.borrow().take.as_ref().unwrap().pattern.hits,
            expected[..2]
        );
        confirm();
        c.close_library().unwrap();
    }
}
