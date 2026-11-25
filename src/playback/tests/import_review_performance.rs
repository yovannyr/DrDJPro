use super::*;
use crate::library::import_review::{inspect, MatchKind};
use std::sync::mpsc;

#[test]
#[ignore = "two 30 s runs with real files and a four-channel output"]
fn r04_equal_duration_import_checks_with_two_playing_decks() {
    let original = wav_file(4096, 48_000 * 4);
    let copy = wav_file(4096, 48_000 * 4);
    let other = wav_file(-4096, 48_000 * 4);
    let files = [original.path(), copy.path(), other.path()].map(|p| p.canonicalize().unwrap());
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        eprintln!("R04 import comparison active={active}");
        let files = &files;
        std::thread::scope(|scope| {
            let (finish, stop) = mpsc::channel::<()>();
            let (ready, started) = mpsc::sync_channel(1);
            let worker = scope.spawn(move || {
                let mut count = 0;
                let cancel = crate::cancellation::Cancellation::default();
                while active && matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                    let batch =
                        crate::library::inspect_paths_cancellable(files.to_vec(), &cancel).unwrap();
                    let findings = inspect(&batch, std::iter::empty(), &cancel).unwrap();
                    assert_eq!(
                        findings
                            .iter()
                            .filter(|f| f.kind == MatchKind::Exact)
                            .count(),
                        2
                    );
                    count += 1;
                    if count == 1 {
                        ready.send(()).unwrap();
                    }
                }
                count
            });
            if active {
                started.recv().unwrap();
            }
            let result = super::render_bench::measure_routed(30, 4, |_| {}, || {}, Some(false));
            drop(finish);
            let scans = worker.join().unwrap();
            assert!(if active { scans >= 2 } else { scans == 0 });
            eprintln!("R04 complete file comparisons={scans}");
            phases.push(result);
        });
    }
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, late blocks, signal gaps, allocations"
    );
}
