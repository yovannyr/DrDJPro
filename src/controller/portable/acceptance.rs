use super::*;
use crate::{controller::tests::finish_storage, test_support::wav_file};
use std::{fs, time::Instant};

fn finish_package(c: &AudioController) {
    let until = Instant::now() + Duration::from_secs(15);
    while c.package.borrow().busy {
        assert!(Instant::now() < until, "package worker timed out");
        c.poll_package();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn m06_preview_and_explicit_save_use_independent_sqlite_catalogs() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let (mixer, _output) = rodio::mixer::mixer(1, 48_000);
    let c = AudioController::wire(
        &ui,
        Rc::new(DualDeckEngine::from_mixer(mixer, 1, 48_000)),
        Arc::new(|| None),
        Arc::new(|| None),
    );
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("studio");
    let target = dir.path().join("stage");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&target).unwrap();
    let recording = |folder: &Path, name: &str, amplitude| {
        let path = folder.join(name);
        fs::copy(wav_file(amplitude, 4800).path(), &path).unwrap();
        path.canonicalize().unwrap()
    };
    let originals = [
        recording(&source, "one.wav", 1000),
        recording(&source, "two.wav", 2000),
        recording(&source, "three.wav", 3000),
    ];
    let library = |folder: &Path, paths: &[PathBuf]| {
        c.restore_library(Ok(folder.join("library.sqlite3")), Vec::new());
        finish_storage(&c);
        if !paths.is_empty() {
            assert_eq!(
                c.library.borrow_mut().insert(paths.iter().cloned()),
                paths.len()
            );
            c.library_changed();
            finish_storage(&c);
        }
        c.package_store().unwrap()
    };
    let origin = library(&source, &originals);
    let catalog = origin.files(0).unwrap();
    assert_eq!(
        catalog.iter().map(|(_, p)| p).collect::<Vec<_>>(),
        originals.iter().collect::<Vec<_>>()
    );
    let entries: Vec<_> = catalog
        .iter()
        .map(|(id, _)| {
            let lib = c.library.borrow();
            let mut entry = Source::from_track(lib.track_by_id(*id).unwrap());
            entry.in_ns = *id as u64 * 5_000_000;
            entry.out_ns = Some(entry.in_ns + 10_000_000);
            entry.cues.push(portable_set::Mark {
                slot: 1,
                ns: entry.in_ns + 1_000_000,
                label: "Mix".into(),
                rgb: 42,
            });
            entry
        })
        .collect();
    let package = dir.path().join("handoff");
    portable_set::export(&entries, "Gig", &package, false, &Cancellation::default()).unwrap();
    assert_eq!(fs::read_dir(&package).unwrap().count(), 1);
    drop(origin);
    c.close_library().unwrap();

    // Each original ID now names a different recording, including a wrong edit of the same size.
    let destination = [
        recording(&target, "three.wav", 3001),
        recording(&target, "renamed-first.wav", 1000),
        recording(&target, "renamed-second.wav", 2000),
    ];
    let sets = library(&target, &destination);
    let target_rows = sets.files(0).unwrap();
    let ids: Vec<_> = catalog.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        target_rows,
        ids.into_iter()
            .zip(destination.iter().cloned())
            .collect::<Vec<_>>()
    );
    let expected = [
        Some(destination[1].clone()),
        Some(destination[2].clone()),
        None,
    ];
    let review = || {
        c.open_package().unwrap();
        finish_package(&c);
        let manifest = package.join("set.json");
        c.package
            .borrow_mut()
            .inspect_manifest(sets.clone(), move || Some(manifest));
        finish_package(&c);
        let state = c.package.borrow();
        let view = state.window.as_ref().unwrap();
        assert!(view.get_imported(), "{}", view.get_status());
        let preview = state.preview.as_ref().unwrap();
        assert_eq!(preview.set.paths, expected);
        assert!(preview.issues[2].contains("missing"));
        for (imported, original) in preview.set.manifest.entries.iter().zip(&entries) {
            assert_eq!(imported.in_ns, original.in_ns);
            assert_eq!(imported.out_ns, original.out_ns);
            assert_eq!(imported.cues[0].ns, original.cues[0].ns);
        }
        view.clone_strong()
    };

    assert!(sets.names().unwrap().is_empty());
    let first = review();
    assert!(sets.names().unwrap().is_empty());
    first.set_set_name("Dismissed".into());
    c.close_package();
    assert!(sets.names().unwrap().is_empty());

    let second = review();
    second.invoke_action(10);
    finish_package(&c);
    assert_eq!(second.get_status(), "Set saved to this library.");
    assert_eq!(sets.names().unwrap(), ["Gig"]);
    c.close_package();
    c.open_package().unwrap();
    finish_package(&c);
    let repeated = c.package.borrow().window.as_ref().unwrap().clone_strong();
    let (release, waiting) = mpsc::channel();
    let manifest = package.join("set.json");
    c.package
        .borrow_mut()
        .inspect_manifest(sets.clone(), move || {
            waiting.recv_timeout(Duration::from_secs(15)).unwrap();
            Some(manifest)
        });
    c.poll_package();
    repeated.invoke_action(13);
    release.send(()).unwrap();
    // Refresh queues behind the cancelled reader and waits for its result to be discarded.
    repeated.invoke_action(15);
    finish_package(&c);
    assert!(!repeated.get_imported());
    assert_eq!(sets.names().unwrap(), ["Gig"]);
    drop(sets);
    c.close_library().unwrap();

    let reopened = library(&target, &[]);
    assert_eq!(reopened.names().unwrap(), ["Gig"]);
    assert_eq!(reopened.read("Gig".into()).unwrap().paths, expected);
    drop(reopened);
    c.close_library().unwrap();
}
