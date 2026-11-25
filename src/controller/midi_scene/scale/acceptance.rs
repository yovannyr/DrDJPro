use super::*;

#[test]
fn r09_scene_callbacks_discard_apply_and_undo_restore_exact_events() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let (mixer, _output) = rodio::mixer::mixer(4, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48_000));
    let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
    let mut original = Scene::default();
    original.events.extend(
        [60, 64, 67, 66, 42]
            .into_iter()
            .enumerate()
            .map(|(i, pitch)| Event {
                note: Note {
                    start: 240 * i as u32,
                    duration: 90 + i as u32,
                    velocity: 70 + i as u8,
                    pitch,
                    channel: if i == 4 { 9 } else { 0 },
                },
                role: if i == 4 { Role::Pad(0) } else { Role::Melody },
                approved: i != 3,
                uncertain: i == 3,
            }),
    );
    original.key = Some("C:maj".into());
    original.tempo_confirmed = true;
    c.scene.borrow_mut().draft = original.clone();
    c.open_midi_scene().unwrap();
    let view = c.scene.borrow().view.as_ref().unwrap().clone_strong();

    view.invoke_propose_scale(0, false, 0, true);
    assert!(view.get_scale_active() && !view.get_scale_ready());
    view.invoke_accept_scale();
    assert!(view.get_scale_active() && !view.get_can_undo());
    assert!(c.scene.borrow().draft == original);
    view.invoke_resolve_scale(0, 65);
    assert!(view.get_scale_ready());
    view.invoke_discard_scale();
    assert!(!view.get_scale_active() && !view.get_can_undo());
    assert!(c.scene.borrow().draft == original);

    for (root, minor, key, pitches) in [
        (0, true, "C:min", [60, 63, 67, 65, 42]),
        (2, false, "D:maj", [62, 66, 69, 67, 42]),
    ] {
        view.invoke_propose_scale(0, false, root, minor);
        view.invoke_resolve_scale(0, i32::from(pitches[3]));
        assert!(view.get_scale_ready() && !view.get_can_undo());
        assert!(c.scene.borrow().draft == original);
        view.invoke_accept_scale();
        let mut expected = original.clone();
        expected.key = Some(key.into());
        for (event, pitch) in expected.events.iter_mut().zip(pitches) {
            event.note.pitch = pitch;
        }
        assert!(
            c.scene.borrow().draft == expected,
            "incorrect scene after applying {key}"
        );
        assert!(!view.get_scale_active() && view.get_can_undo());
        view.invoke_undo_scene();
        assert!(
            c.scene.borrow().draft == original,
            "undo changed the original scene after {key}"
        );
        assert!(!view.get_can_undo());
    }
    view.invoke_dismiss();
}
