//! Production Slint contract tests, without devices or persistent user data.
use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc};

use drdjpro_ui_spike::*;

fn click(ui: &AppWindow, label: &str) {
    let controls: Vec<_> = ElementHandle::find_by_accessible_label(ui, label)
        .filter(|item| {
            item.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button)
        })
        .collect();
    assert_eq!(controls.len(), 1, "unique action: {label}");
    controls[0].invoke_accessible_default_action();
}

#[test]
fn cover_labels_show_confirmed_catalog_values_in_both_layouts() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.window().set_size(slint::LogicalSize::new(850., 860.));
    ui.set_table_columns(
        Rc::new(slint::VecModel::from(vec![TableColumn {
            id: 11,
            title: "Cover".into(),
            width: 36.,
            visible: true,
            ..Default::default()
        }]))
        .into(),
    );
    ui.set_table_width(36.);
    let image = slint::Image::from_rgb8(slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(8, 4));
    let rows = [
        LibraryRow {
            id: 1,
            cover: image.clone(),
            cover_key: "12B".into(),
            cover_energy: "10".into(),
            ..Default::default()
        },
        LibraryRow {
            id: 2,
            cover_key: "8A".into(),
            ..Default::default()
        },
        LibraryRow {
            id: 3,
            key: "7B".into(),
            ..Default::default()
        },
    ];
    ui.set_library_rows(Rc::new(slint::VecModel::from(rows.to_vec())).into());
    let toggle = ui.as_weak();
    ui.on_toggle_cover_overlay(move || {
        let ui = toggle.unwrap();
        ui.set_cover_overlay_enabled(!ui.get_cover_overlay_enabled());
    });
    ui.show().unwrap();
    for mode in [LayoutMode::Classic, LayoutMode::Modern] {
        ui.set_view_mode(mode);
        for selected in [1, 2] {
            ui.set_selected_track(selected);
            let labels = |id| {
                ElementHandle::find_by_accessible_label(&ui, &format!("Track {id} cover labels"))
                    .next()
            };
            let badge = labels(1).unwrap();
            let desc = badge.accessible_description().unwrap();
            assert!(desc.starts_with("Camelot 12B, energy 10"));
            assert_eq!(desc.ends_with(", selected"), selected == 1);
            let bounds = |element: &ElementHandle| {
                let p = element.absolute_position();
                let size = element.size();
                assert!(size.width > 0. && size.height > 0., "{size:?}");
                (p.x, p.y, p.x + size.width, p.y + size.height)
            };
            let contains = |outer: (f32, f32, f32, f32), inner: (f32, f32, f32, f32)| {
                outer.0 <= inner.0 && outer.1 <= inner.1 && inner.2 <= outer.2 && inner.3 <= outer.3
            };
            let chips = badge
                .query_descendants()
                .match_type_name("CoverBadge")
                .find_all();
            assert_eq!(chips.len(), 2);
            for chip in &chips {
                let rect = bounds(chip);
                let caption = chip
                    .query_descendants()
                    .match_type_name("Text")
                    .find_first()
                    .unwrap();
                let ink = bounds(&caption);
                assert!(
                    contains(bounds(&badge), rect),
                    "Cover chip outside cell: {rect:?}"
                );
                assert!(
                    contains(rect, ink),
                    "Caption {ink:?} must fit its chip {rect:?}"
                );
            }
            let (first, second) = (bounds(&chips[0]), bounds(&chips[1]));
            assert!(
                first.2 <= second.0
                    || second.2 <= first.0
                    || first.3 <= second.1
                    || second.3 <= first.1,
                "Overlapping chips: {first:?}, {second:?}"
            );
            let second = labels(2).unwrap().accessible_description().unwrap();
            assert!(second.starts_with("Camelot 8A, energy unknown"));
            assert_eq!(second.ends_with(", selected"), selected == 2);
            assert!(labels(3).is_none(), "unconfirmed estimated key");
            assert!(
                ElementHandle::find_by_accessible_label(&ui, "Track 2 cover placeholder")
                    .next()
                    .is_some()
            );
            assert!(
                ElementHandle::find_by_accessible_label(&ui, "Track 1 cover placeholder")
                    .next()
                    .is_none()
            );
        }
        let picture = || {
            ElementHandle::find_by_accessible_label(&ui, "Track 1 cover art")
                .next()
                .unwrap()
                .size()
        };
        let original = picture();
        click(&ui, "Configure track columns");
        let toggle = ElementHandle::find_by_accessible_label(&ui, "Show key and energy on covers")
            .next()
            .unwrap();
        toggle.invoke_accessible_default_action();
        assert!(!ui.get_cover_overlay_enabled());
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Track 1 cover labels")
                .next()
                .is_none()
        );
        assert_eq!(picture(), original);
        assert_eq!(ui.get_selected_track(), 2);
        toggle.invoke_accessible_default_action();
        click(&ui, "Close track columns");
    }
}

#[test]
fn cover_label_editor_preserves_typed_values_and_cancels_on_track_change() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.set_library_tab(4);
    ui.set_selected_track(7);
    ui.show().unwrap();
    let target = ui.as_weak();
    ui.on_open_cover_labels(move || {
        target.unwrap().set_cover_label_editor(CoverLabelEditor {
            open: true,
            track_id: 7,
            key: "8A".into(),
            ..Default::default()
        })
    });
    click(&ui, "Edit confirmed cover labels");
    let control = |label| {
        ElementHandle::find_by_accessible_label(&ui, label)
            .next()
            .unwrap()
    };
    control("Confirmed Camelot key").set_accessible_value("9B");
    control("Confirmed energy").set_accessible_value("11");
    let target = ui.as_weak();
    ui.on_save_cover_labels(move |id, key, energy| {
        assert_eq!((id, key.as_str(), energy.as_str()), (7, "9B", "11"));
        target.unwrap().set_cover_label_editor(CoverLabelEditor {
            open: true,
            track_id: id,
            key,
            energy,
            status: "Energy must be 1–10".into(),
        });
    });
    click(&ui, "Save cover labels");
    assert_eq!(
        control("Cover label status")
            .accessible_description()
            .as_deref(),
        Some("Energy must be 1–10")
    );
    assert_eq!(
        control("Confirmed energy").accessible_value().as_deref(),
        Some("11")
    );
    let target = ui.as_weak();
    ui.on_cancel_cover_labels(move || target.unwrap().set_cover_label_editor(Default::default()));
    ui.set_selected_track(8);
    assert_eq!(
        control("Save cover labels").accessible_enabled(),
        Some(false)
    );
    slint::platform::update_timers_and_animations();
    assert!(!ui.get_cover_label_editor().open);
}

#[test]
fn prepare_tempo_comparison_exposes_variants_and_protects_the_grid_in_both_layouts() {
    i_slint_backend_testing::init_no_event_loop();
    for mode in [LayoutMode::Classic, LayoutMode::Modern] {
        let ui = AppWindow::new().unwrap();
        ui.set_view_mode(mode);
        ui.set_library_tab(4);
        ui.set_selected_track(1);
        ui.set_audio_enabled(true);
        ui.window().set_size(slint::LogicalSize::new(850., 860.));
        ui.show().unwrap();
        let opened = Rc::new(Cell::new(false));
        let observed = opened.clone();
        ui.on_open_tempo_comparison(move || observed.set(true));
        click(&ui, "Compare half and double tempo");
        assert!(opened.get());

        let mut comparison = TempoComparisonView {
            open: true,
            track_id: 1,
            slow_label: "85 BPM".into(),
            fast_label: "170 BPM".into(),
            can_toggle_lock: true,
            locked: true,
            status: "Grid locked".into(),
            anchor_label: "Anchor 1.000 s".into(),
            beats: Rc::new(slint::VecModel::from(vec![
                BeatLine {
                    downbeat: true,
                    ..Default::default()
                },
                BeatLine {
                    position: 0.5,
                    ..Default::default()
                },
            ]))
            .into(),
            ..Default::default()
        };
        ui.set_tempo_comparison(comparison.clone());
        let fast = Rc::new(Cell::new(None));
        let observed = fast.clone();
        ui.on_select_tempo_variant(move |value| observed.set(Some(value)));
        click(&ui, "Faster tempo interpretation");
        assert_eq!(fast.get(), Some(true));
        click(&ui, "Slower tempo interpretation");
        assert_eq!(fast.get(), Some(false));
        let apply_count = Rc::new(Cell::new(0));
        let observed = apply_count.clone();
        ui.on_apply_tempo_comparison(move || observed.set(observed.get() + 1));
        click(&ui, "Apply tempo interpretation");
        assert_eq!(apply_count.get(), 0);
        let lock = Rc::new(Cell::new(false));
        let observed = lock.clone();
        ui.on_toggle_tempo_grid_lock(move || observed.set(true));
        click(&ui, "Toggle tempo grid lock");
        assert!(lock.get());

        comparison.locked = false;
        comparison.can_apply = true;
        comparison.fast_label = "".into();
        ui.set_tempo_comparison(comparison);
        click(&ui, "Faster tempo interpretation");
        assert_eq!(fast.get(), Some(false));
        click(&ui, "Apply tempo interpretation");
        assert_eq!(apply_count.get(), 1);
        let beats = ElementHandle::find_by_accessible_label(&ui, "Tempo comparison beat marks")
            .next()
            .unwrap();
        assert_eq!(
            beats.accessible_description().as_deref(),
            Some("Anchor 1.000 s")
        );
        let mut marks = beats
            .query_descendants()
            .match_type_name("Rectangle")
            .find_all();
        marks.sort_by(|a, b| a.absolute_position().x.total_cmp(&b.absolute_position().x));
        assert_eq!(marks.len(), 2);
        assert!(marks[0].size().height > marks[1].size().height);
        assert!((marks[0].absolute_position().x - beats.absolute_position().x).abs() < 1.);
        let middle = beats.absolute_position().x + beats.size().width * 0.5;
        assert!((marks[1].absolute_position().x - middle).abs() <= 1.);
    }
}

#[test]
fn prepare_tempo_comparison_cancels_when_the_user_leaves_the_preview() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.set_view_mode(LayoutMode::Modern);
    ui.show().unwrap();
    let cancellations = Rc::new(Cell::new(0));
    let observed = cancellations.clone();
    let weak = ui.as_weak();
    ui.on_cancel_tempo_comparison(move || {
        observed.set(observed.get() + 1);
        weak.unwrap().set_tempo_comparison(Default::default());
    });
    for exit in 0..5 {
        ui.set_library_tab(4);
        ui.set_selected_track(1);
        ui.set_modern_library_open(true);
        slint::platform::update_timers_and_animations();
        ui.set_tempo_comparison(TempoComparisonView {
            open: true,
            track_id: 1,
            ..Default::default()
        });
        match exit {
            0 => click(&ui, "Cancel tempo comparison"),
            1 => ui.set_library_tab(0),
            2 => ui.set_modern_library_open(false),
            3 => ui.set_selected_track(2),
            _ => click(&ui, "Open settings"),
        }
        slint::platform::update_timers_and_animations();
        assert_eq!(cancellations.get(), exit + 1);
        assert!(!ui.get_tempo_comparison().open);
    }
}

#[test]
fn settings_routes_preserve_playback_and_keep_live_shortcuts_out_of_setup() {
    use slint::platform::{Key, WindowEvent};
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.window().set_size(slint::LogicalSize::new(850., 860.));
    ui.set_left_loaded(true);
    ui.set_left_grid_active(true);
    ui.set_left_playing(true);
    ui.set_selected_track(9);
    let imported = Rc::new(Cell::new(false));
    let observed = imported.clone();
    ui.on_show_itunes(move || observed.set(true));
    let help_requests = Rc::new(Cell::new(0));
    let observed = help_requests.clone();
    ui.on_show_browser(move |help| {
        assert!(help);
        observed.set(observed.get() + 1);
    });
    let cues = Rc::new(Cell::new(0));
    let observed = cues.clone();
    ui.on_hold_hot_cue_left(move |_, _, _| observed.set(observed.get() + 1));
    ui.show().unwrap();
    for mode in [LayoutMode::Classic, LayoutMode::Modern] {
        ui.set_view_mode(mode);
        let dest = ElementHandle::find_by_accessible_label(&ui, "DECK B load track")
            .next()
            .unwrap()
            .absolute_position();
        assert_eq!(ui.invoke_external_drop_target(dest.x + 2., dest.y + 2.), 1);
        click(&ui, "Open settings");
        click(&ui, "Settings: Decks");
        let loops = ElementHandle::find_by_accessible_label(&ui, "Deck A snap loops in settings")
            .next()
            .unwrap();
        assert!(loops.absolute_position().x + loops.size().width <= 850.);
        let before = ui.get_left_auto_loop_snap();
        loops.invoke_accessible_default_action();
        assert_eq!(ui.get_left_auto_loop_snap(), !before);
        assert_eq!(ui.invoke_external_drop_target(dest.x + 2., dest.y + 2.), -1);
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: "1".into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: "1".into() });
        assert_eq!(cues.get(), 0);
        click(&ui, "Settings: Audio");
        assert!(ui.get_audio_device_open());
        let audio_tab = ElementHandle::find_by_accessible_label(&ui, "Settings: Audio")
            .next()
            .unwrap();
        assert_eq!(
            audio_tab.accessible_description().as_deref(),
            Some("Ausgewählter Einstellungsbereich")
        );
        click(&ui, "Settings: Mixer");
        assert!(ui.get_monitor_open() && !ui.get_audio_device_open());
        click(&ui, "Settings: Controller");
        assert!(ui.get_usb_finder_open() && !ui.get_monitor_open());
        click(&ui, "Settings: Library & Ansicht");
        assert!(!ui.get_usb_finder_open());
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Performance layout")
                .next()
                .is_some()
        );
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Escape.into(),
        });
        assert_eq!(ui.invoke_external_drop_target(dest.x + 2., dest.y + 2.), 1);
        assert!(ui.get_left_playing());
        assert_eq!(ui.get_selected_track(), 9);
        click(&ui, "More actions");
        click(&ui, "Hilfe öffnen");
        assert!(ElementHandle::find_by_accessible_label(&ui, "Hilfe öffnen")
            .next()
            .is_none());
    }
    assert_eq!(help_requests.get(), 2);
    ui.set_library_tab(1);
    click(&ui, "Open iTunes Music XML library");
    assert!(imported.get());
    ui.hide().unwrap();
}

#[test]
fn layout_modes_preserve_deck_and_library_state_and_route_independently() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.set_left_loaded(true);
    ui.set_right_loaded(true);
    ui.set_left_title("A: supplied controller metadata".into());
    ui.set_right_title("B: supplied controller metadata".into());
    ui.set_left_position(23.5);
    ui.set_right_position(72.0);
    ui.set_left_pitch_percent(-3.0);
    ui.set_right_pitch_percent(6.0);
    ui.set_selected_track(17);
    ui.set_library_query("supplied query".into());
    ui.set_left_playing(true);
    ui.set_left_sync_active(true);
    ui.set_left_keylock(true);
    let a = Rc::new(Cell::new(0));
    let b = Rc::new(Cell::new(0));
    let count = a.clone();
    ui.on_toggle_left(move || count.set(count.get() + 1));
    let count = b.clone();
    ui.on_toggle_right(move || count.set(count.get() + 1));
    ui.show().unwrap();
    for _ in 0..3 {
        for (label, slots) in [("4 SLOTS", 4), ("2 DECKS", 2)] {
            click(&ui, label);
            assert_eq!(ui.get_visible_layout_slots(), slots);
            assert_eq!(ui.get_supported_audio_deck_count(), 2);
            assert_eq!(ui.get_left_position(), 23.5);
            assert_eq!(ui.get_right_position(), 72.0);
            assert_eq!(ui.get_selected_track(), 17);
            assert_eq!(ui.get_library_query(), "supplied query");
            assert_eq!(ui.get_left_pitch_percent(), -3.0);
            assert_eq!(ui.get_right_pitch_percent(), 6.0);
            assert!(ui.get_left_playing());
            assert!(ui.get_left_sync_active());
            assert!(ui.get_left_keylock());
            assert_eq!(ui.get_right_title(), "B: supplied controller metadata");
            assert!(ui.get_deck_c_data().title.is_empty());
            assert!(ui.get_deck_d_data().title.is_empty());
        }
    }
    click(&ui, "DECK A play/pause");
    assert_eq!((a.get(), b.get()), (1, 0));
    click(&ui, "DECK B play/pause");
    assert_eq!((a.get(), b.get()), (1, 1));
    // Controls dispatch requests; they never optimistically change engine state.
    assert!(ui.get_left_playing());
    assert!(!ui.get_right_playing());
}

#[test]
fn production_layout_geometry_keeps_transport_and_waveforms_reachable() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_mc4000_connected(true);
    ui.show().unwrap();
    for (four, width, height) in [
        (false, 1280., 860.),
        (false, 850., 860.),
        (true, 850., 1020.),
    ] {
        ui.set_four_decks(four);
        ui.window().set_size(slint::LogicalSize::new(width, height));
        ui.set_deck_view_index(1);
        for label in [
            "DECK A play/pause",
            "DECK B play/pause",
            "Shared waveform area",
            "Track library area",
        ] {
            let item = ElementHandle::find_by_accessible_label(&ui, label)
                .next()
                .unwrap();
            let pos = item.absolute_position();
            let size = item.size();
            assert!(size.width > 0. && size.height > 0., "{label}");
            assert!(pos.x >= 0. && pos.y >= 0., "{label}");
            assert!(pos.x + size.width <= width + 1., "{label} width");
            assert!(pos.y + size.height <= height + 1., "{label} height");
        }
        let wave = ElementHandle::find_by_accessible_label(&ui, "Shared waveform area")
            .next()
            .unwrap();
        for deck in ["DECK A", "DECK B"] {
            for suffix in [
                "virtual deck source time",
                "virtual deck BPM",
                "virtual deck pitch",
            ] {
                let label = format!("{deck} {suffix}");
                let item = ElementHandle::find_by_accessible_label(&ui, &label)
                    .next()
                    .unwrap();
                let pos = item.absolute_position();
                let size = item.size();
                assert!(
                    pos.y + size.height <= wave.absolute_position().y,
                    "{label} overlaps waveforms"
                );
            }
        }
    }
}

#[test]
fn unsupported_slots_have_no_transport_routes_and_editors_stay_reachable() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_four_decks(true);
    ui.set_audio_enabled(true);
    ui.set_left_loaded(true);
    ui.set_right_loaded(true);
    ui.show().unwrap();
    for deck in ["DECK C", "DECK D"] {
        assert!(
            ElementHandle::find_by_accessible_label(&ui, &format!("{deck} play/pause"))
                .next()
                .is_none()
        );
        assert!(
            ElementHandle::find_by_accessible_label(&ui, &format!("{deck} load track"))
                .next()
                .is_none()
        );
    }
    // Traverse the real tools action, including the new cue-editor page.
    for _ in 0..5 {
        click(&ui, "DECK A deck tools panel");
    }
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "DECK A edit hot cue")
            .next()
            .is_some()
    );
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "DECK A stutter")
            .next()
            .is_some()
    );
    ui.set_left_cue_editing(true);
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "DECK A cue name")
            .next()
            .is_some()
    );
    click(&ui, "2 DECKS");
    assert!(ui.get_left_cue_editing());
    assert!(
        ElementHandle::find_by_accessible_label(&ui, "DECK A cue name")
            .next()
            .is_some()
    );
}

#[test]
fn modern_platter_accessible_data_binding() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.set_left_loaded(true);
    ui.set_right_loaded(true);
    ui.set_four_decks(true);
    ui.set_view_mode(LayoutMode::Modern);
    ui.show().unwrap();
    ui.set_left_platter_bpm("128.4".into());
    ui.set_right_platter_bpm("120.2".into());
    ui.set_left_platter_time("03:24".into());
    ui.set_right_platter_time("02:50".into());
    ui.set_left_platter_angle(45.0);
    ui.set_right_platter_angle(-30.0);
    ui.set_left_pitch_percent(5.5);
    ui.set_right_pitch_percent(-2.3);
    ui.set_left_position(25.0);
    ui.set_right_position(75.0);
    ui.set_left_playing(true);
    ui.set_right_playing(false);
    let a = ElementHandle::find_by_accessible_label(&ui, "Deck A modern platter").next();
    let b = ElementHandle::find_by_accessible_label(&ui, "Deck B modern platter").next();
    assert!(a.is_some(), "A");
    assert!(b.is_some(), "B");
    if let Some(ref item) = a {
        let desc = item.accessible_description().unwrap_or_default();
        let desc: &str = desc.as_str();
        assert!(desc.contains("128.4"), "A bpm");
        assert!(desc.contains("03:24"), "A time");
        assert!(desc.contains("pitch +5.5%"), "A pitch");
        assert!(desc.contains("25%"), "A pos");
        assert!(desc.contains("45"), "A angle");
        assert!(desc.contains("Playing"), "A play");
    }
    if let Some(ref item) = b {
        let desc = item.accessible_description().unwrap_or_default();
        let desc: &str = desc.as_str();
        assert!(desc.contains("120.2"), "B bpm");
        assert!(desc.contains("02:50"), "B time");
        assert!(desc.contains("-2.3%"), "B pitch");
        assert!(desc.contains("75%"), "B pos");
        assert!(desc.contains("-30"), "B angle");
        assert!(desc.contains("Paused"), "B paused");
    }
    ui.set_left_playing(false);
    if let Some(ref item) = a {
        let desc = item.accessible_description().unwrap_or_default();
        let desc: &str = desc.as_str();
        assert!(desc.contains("Paused"), "pause");
    }

    // C/D: platters exist in four-decks mode; in-only props, no Rust setters
    let c_item = ElementHandle::find_by_accessible_label(&ui, "Deck C modern platter").next();
    let d_item = ElementHandle::find_by_accessible_label(&ui, "Deck D modern platter").next();
    assert!(c_item.is_some(), "C platter exists in four-decks");
    assert!(d_item.is_some(), "D platter exists in four-decks");
}

#[test]
fn modern_scratch_releases_on_right_button_up_and_focus_loss() {
    use slint::platform::{PointerEventButton, WindowEvent};
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.set_audio_enabled(true);
    ui.set_left_loaded(true);
    ui.set_right_loaded(true);
    ui.set_view_mode(LayoutMode::Modern);
    ui.show().unwrap();
    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));

    let begin_count = Rc::new(Cell::new(0));
    let release_count = Rc::new(Cell::new(0));
    let begin_count_r = begin_count.clone();
    let release_count_r = release_count.clone();
    ui.on_virtual_platter_pointer(move |_deck: i32, action: i32, _x: f32, _y: f32| {
        if action == 0 {
            begin_count_r.set(begin_count_r.get() + 1);
        } else if action == 2 {
            release_count_r.set(release_count_r.get() + 1);
        }
    });

    let platter_a = ElementHandle::find_by_accessible_label(&ui, "Deck A modern platter")
        .next()
        .expect("Deck A modern platter exists");
    let origin = platter_a.absolute_position();
    let size = platter_a.size();
    let center =
        slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    assert_eq!(begin_count.get(), 0);
    assert_eq!(release_count.get(), 0);

    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: center,
        button: PointerEventButton::Left,
    });
    assert_eq!(begin_count.get(), 1, "begin on left-button down");

    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: center,
        button: PointerEventButton::Right,
    });
    assert_eq!(release_count.get(), 1, "release on right-button up");
    assert_eq!(begin_count.get(), 1, "no double begin");

    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: center,
        button: PointerEventButton::Left,
    });
    assert_eq!(begin_count.get(), 2, "second begin");
    assert_eq!(release_count.get(), 1, "still waiting for release");

    ui.window()
        .dispatch_event(WindowEvent::WindowActiveChanged(false));
    assert_eq!(release_count.get(), 2, "release on focus loss");
    assert_eq!(begin_count.get(), 2, "no double begin on second attempt");
}
