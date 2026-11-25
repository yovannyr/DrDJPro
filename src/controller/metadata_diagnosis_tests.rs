fn completed_diagnosis(c: &AudioController, ui: &AppWindow, id: i32) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        c.tick();
        let view = ui.get_metadata_diagnosis();
        if view.open && view.track_id == id && view.text.contains("Effective catalog") {
            return view.text.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "Diagnosis did not finish: {}",
            view.text
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct ReadOnlyFile<'a>(&'a Path, std::fs::Permissions);

impl<'a> ReadOnlyFile<'a> {
    fn new(path: &'a Path) -> Self {
        let original = std::fs::metadata(path).unwrap().permissions();
        let mut restricted = original.clone();
        restricted.set_readonly(true);
        std::fs::set_permissions(path, restricted).unwrap();
        Self(path, original)
    }
}

impl Drop for ReadOnlyFile<'_> {
    fn drop(&mut self) {
        std::fs::set_permissions(self.0, self.1.clone()).unwrap();
    }
}

#[test]
fn metadata_diagnosis_recognizes_supported_audio_containers() {
    use crate::cancellation::Cancellation;
    use crate::track_metadata::diagnosis::{diagnose, CatalogValues};
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for file in [
        "format-aac.m4a",
        "format-alac.m4a",
        "format-flac.flac",
        "format-aiff.aiff",
        "format-vorbis.ogg",
        "format-opus.opus",
        "format-opus-mono.opus",
        "tone-9s.mp3",
    ] {
        let report = diagnose(
            &fixtures.join(file),
            &CatalogValues::default(),
            &Cancellation::default(),
        )
        .unwrap();
        assert!(
            !report.text.contains("unreadable"),
            "{file}: {}",
            report.text
        );
    }
}

#[test]
fn metadata_diagnosis_preserves_read_only_audio_and_catalog_in_both_layouts() {
    use crate::test_support::tagged_diagnostic_mp3;
    use crate::track_metadata::MusicLabels;
    use sha2::{Digest, Sha256};
    i_slint_backend_testing::init_no_event_loop();
    let slow = tagged_diagnostic_mp3("85", "");
    let harmonic = tagged_diagnostic_mp3("122", "Am");
    let files = [slow.path(), harmonic.path()].map(|path| path.canonicalize().unwrap());
    let _permissions = files.each_ref().map(|path| ReadOnlyFile::new(path));
    let hashes = || {
        files
            .each_ref()
            .map(|p| Sha256::digest(std::fs::read(p).unwrap()))
    };
    let before = hashes();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("diagnosis.sqlite3");
    let (mixer, _output) = rodio::mixer::mixer(2, 48_000);
    let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 2, 48_000));
    let ui = AppWindow::new().unwrap();
    let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
    c.restore_library(Ok(db.clone()), vec![]);
    finish_storage(&c);
    let grid = crate::beatgrid::BeatGrid::new(170., Duration::from_millis(50)).unwrap();
    let labels = MusicLabels::parse("11B", "8").unwrap();
    let ids = {
        let mut library = c.library.borrow_mut();
        assert_eq!(library.insert(files.iter().cloned()), 2);
        let ids = files
            .each_ref()
            .map(|p| library.track_id_for_path(p).unwrap());
        library.set_grid(&files[0], Some(grid)).unwrap();
        library.set_music_labels(ids[1], labels).unwrap();
        ids
    };
    c.library_changed();
    c.publish_library();
    finish_storage(&c);
    ui.invoke_library_tab_selected(4);
    finish_search(&c);
    for layout in [crate::LayoutMode::Classic, crate::LayoutMode::Modern] {
        ui.set_view_mode(layout);
        ui.set_modern_library_open(true);
        ui.invoke_select_library(ids[0]);
        ui.invoke_open_metadata_diagnosis();
        ui.invoke_select_library(ids[1]);
        ui.invoke_select_library(ids[0]);
        c.tick();
        assert!(
            !ui.get_metadata_diagnosis().open,
            "obsolete diagnosis resurfaced"
        );
        ui.invoke_open_metadata_diagnosis();
        let report = completed_diagnosis(&c, &ui, ids[0]);
        let (tags, remaining) = report.split_once("Last audio analysis").unwrap();
        let (analysis, effective) = remaining.split_once("Effective catalog").unwrap();
        assert!(tags.contains("File tags") && tags.contains("85"), "{tags}");
        assert!(
            tags.lines().any(|line| line == "File attribute: read-only"),
            "{tags}"
        );
        assert!(analysis.contains("No analysis available"), "{analysis}");
        assert!(effective.contains("170"), "{effective}");
        assert!(ui.get_metadata_diagnosis().compare);
        ui.invoke_close_metadata_diagnosis();
        assert!(!ui.get_metadata_diagnosis().open);
        ui.invoke_select_library(ids[1]);
        ui.invoke_open_metadata_diagnosis();
        let report = completed_diagnosis(&c, &ui, ids[1]);
        let (tags, remaining) = report.split_once("Last audio analysis").unwrap();
        assert!(tags.contains("Am"), "{tags}");
        assert!(
            remaining
                .split_once("Effective catalog")
                .unwrap()
                .1
                .contains("11B"),
            "{remaining}"
        );
        assert!(!ui.get_metadata_diagnosis().compare);
        ui.invoke_close_metadata_diagnosis();
    }
    assert_eq!(hashes(), before);
    assert!(files
        .iter()
        .all(|p| std::fs::metadata(p).unwrap().permissions().readonly()));
    c.close_library().unwrap();
    let store = crate::library_store::Worker::start(db).unwrap();
    let crate::library_store::Event::Loaded(Ok(saved)) = store.events.recv().unwrap() else {
        panic!("expected the persisted library");
    };
    assert_eq!(saved.library.grid(&files[0]), Some(grid));
    let restored = saved.library.track_by_id(ids[1]).unwrap();
    assert_eq!(restored.metadata.music_labels, labels);
    assert!(restored.metadata.analysis.is_none());
    store.close_delta(None).unwrap();
}
