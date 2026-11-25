use super::*;
use crate::{
    hot_cues::HotCue,
    test_support::{wav_file, wav_samples},
};
use std::time::Instant;

#[test]
fn a05_stale_prepare_worker_is_discarded_while_the_deck_keeps_playing() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let (mixer, mut output) = rodio::mixer::mixer(2, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000));
    let c = AudioController::wire(&ui, engine.clone(), Arc::new(|| None), Arc::new(|| None));
    let samples: Vec<_> = (0..48_000)
        .map(|i| (12_000.0 * (std::f32::consts::TAU * i as f32 * 220.0 / 48_000.0).sin()) as i16)
        .collect();
    let tone = wav_samples(&samples, 48_000, 1);
    let silence = wav_file(0, 48_000);
    let a = tone.path().canonicalize().unwrap();
    let b = silence.path().canonicalize().unwrap();
    let (id_a, id_b) = {
        let mut lib = c.library.borrow_mut();
        lib.insert([a.clone(), b.clone()]);
        lib.set_hot_cue(
            &a,
            0,
            Some(HotCue::new(Duration::from_millis(600), "Entry".into(), 0xabcdef).unwrap()),
        )
        .unwrap();
        (
            lib.track_id_for_path(&a).unwrap(),
            lib.track_id_for_path(&b).unwrap(),
        )
    };
    let cues = c.library.borrow().hot_cues(&a);
    let select = |id| {
        c.library.borrow_mut().select(id);
        c.publish_selected_library_track(&ui);
    };
    engine.load_a(&a).unwrap();
    engine.play(DeckId::A);
    let peak = output
        .by_ref()
        .take(2000)
        .map(f32::abs)
        .fold(0.0_f32, f32::max);
    assert!(peak > 0.001);
    let playing = engine.snapshot(DeckId::A);
    assert!(playing.playing && playing.position > Duration::ZERO);

    ui.set_view_mode(crate::LayoutMode::Classic);
    ui.set_library_tab(4);
    ui.set_left_detail_window(16);
    ui.set_split_waveform(true);
    select(id_a);
    c.poll_layer_view();
    // Hold the real worker result until the selection has changed. No timing race or fake DSP.
    let (relay, rx) = mpsc::channel();
    let old_rx = std::mem::replace(&mut c.layer_view.borrow_mut().job.as_mut().unwrap().rx, rx);
    let deadline = Duration::from_secs(15);
    assert!(matches!(
        old_rx.recv_timeout(deadline).unwrap(),
        Update::Loading
    ));
    let Update::Ready(values) = old_rx.recv_timeout(deadline).unwrap() else {
        panic!("tonal worker did not produce a waveform");
    };
    assert!(values.iter().any(|v| v.tonal > 0.2));
    select(id_b);
    relay.send(Update::Ready(values)).unwrap();
    drop(relay);
    c.poll_layer_view();
    assert_eq!(ui.get_layer_percussion().row_count(), 0);
    assert_eq!(ui.get_layer_tonal().row_count(), 0);
    let await_silence = || {
        let start = Instant::now();
        while ui.get_layer_tonal().row_count() == 0 {
            assert!(start.elapsed() < deadline, "{}", ui.get_layer_status());
            std::thread::sleep(Duration::from_millis(1));
            c.poll_layer_view();
        }
        let tonal = ui.get_layer_tonal();
        let drums = ui.get_layer_percussion();
        assert_eq!(drums.row_count(), tonal.row_count());
        assert!(drums.iter().chain(tonal.iter()).all(|v| v == 0.0));
    };
    await_silence();
    // The last published result must disappear on selection, even before a UI timer tick.
    select(id_a);
    assert_eq!(ui.get_layer_tonal().row_count(), 0);
    assert_eq!(ui.get_layer_percussion().row_count(), 0);
    select(id_b);
    await_silence();

    ui.set_split_waveform(false);
    c.poll_layer_view();
    assert_eq!(ui.get_layer_tonal().row_count(), 0);
    assert_eq!(ui.get_selected_track(), id_b);
    assert_eq!(ui.get_left_detail_window(), 16);
    assert_eq!(c.library.borrow().hot_cues(&a), cues);
    let after = engine.snapshot(DeckId::A);
    assert!(after.playing);
    assert_eq!(after.path, playing.path);
    assert_eq!(after.position, playing.position);
    for _ in 0..2000 {
        output.next().unwrap();
    }
    assert!(engine.snapshot(DeckId::A).position > after.position);
}
