use super::*;
use crate::track_metadata::diagnosis::{diagnose, CatalogValues};
use std::sync::mpsc;

#[test]
#[ignore = "two 30 s measurements with file-header reads and four-channel audio"]
fn r05_equal_duration_metadata_diagnosis_with_two_decks() {
    let source = crate::test_support::tagged_diagnostic_mp3("85", "Am");
    let path = source.path().canonicalize().unwrap();
    let mut library = crate::library::Library::default();
    library.insert([path.clone()]);
    library
        .set_grid(
            &path,
            Some(crate::beatgrid::BeatGrid::new(170., Duration::ZERO).unwrap()),
        )
        .unwrap();
    let catalog = CatalogValues::from_track(
        library
            .track_by_id(library.track_id_for_path(&path).unwrap())
            .unwrap(),
    );
    for active in [false, true] {
        let (stop, stopping) = mpsc::channel::<()>();
        let (ready, started) = mpsc::sync_channel(1);
        let catalog = &catalog;
        let path = &path;
        std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                if !active {
                    return 0;
                }
                let cancel = crate::cancellation::Cancellation::default();
                let mut reads = 0;
                loop {
                    let report = diagnose(path, catalog, &cancel).unwrap();
                    assert!(report.compare && report.text.contains("170"));
                    reads += 1;
                    if reads == 1 {
                        ready.send(()).unwrap();
                    }
                    if stopping.recv_timeout(Duration::from_millis(100))
                        != Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        break;
                    }
                }
                reads
            });
            if active {
                started.recv().unwrap();
            }
            eprintln!("R05 metadata diagnosis active={active}");
            let result = super::render_bench::measure_routed(30, 4, |_| {}, || {}, Some(false));
            drop(stop);
            let reads = worker.join().unwrap();
            eprintln!("R05 completed diagnoses={reads}");
            assert!(if active { reads >= 200 } else { reads == 0 });
            assert_eq!(result, (0, 0, 0, 0));
        });
    }
}
