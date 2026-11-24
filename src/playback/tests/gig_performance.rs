use crate::{
    library::{gigs::GigStatus, Library},
    library_store::{Event, Worker},
    test_support::wav_file,
};
use std::{path::Path, sync::mpsc, time::Duration};

fn open_gig_store(path: &Path) -> (Worker, Library) {
    let store = Worker::start(path.to_path_buf()).unwrap();
    match store.events.recv_timeout(Duration::from_secs(10)).unwrap() {
        Event::Loaded(Ok(saved)) => (store, saved.library),
        Event::Loaded(Err(error)) => panic!("Cannot open gig measurement library: {error}"),
        _ => panic!("Expected library load"),
    }
}

fn save_changes(library: &mut Library, store: &Worker) {
    let delta = library.changes();
    let revision = delta.revision;
    store.save_delta(delta).unwrap();
    match store.events.recv_timeout(Duration::from_secs(10)).unwrap() {
        Event::Saved(result) => result.unwrap(),
        _ => panic!("Expected acknowledgement of the gig changes"),
    }
    library.acknowledge(revision);
    assert!(!library.has_changes());
}

fn gig_cycle(library: &mut Library, store: &Worker, gigs: [i32; 2], track: i32, cycle: u64) {
    let index = ((cycle - 1) % 2) as usize;
    let (active, other) = (gigs[index], gigs[1 - index]);
    library.select_gig(Some(active)).unwrap();
    library
        .set_gig_rule(active, track, Some(GigStatus::Required), "Opening set")
        .unwrap();
    library
        .set_gig_rule(
            other,
            track,
            Some(GigStatus::Avoid),
            "Reserved for the other room",
        )
        .unwrap();
    assert!(library.record_play(track, cycle, 0).unwrap());
    assert_eq!(
        library.gig_rule(active, track).unwrap().fulfilled_at_ms,
        Some(cycle)
    );
    assert_eq!(
        library.gig_rule(other, track).unwrap().fulfilled_at_ms,
        None
    );
    save_changes(library, store);
}

#[test]
#[ignore = "two 30 s measurements with four-channel audio and persisted gig rule changes"]
fn r02_equal_duration_gig_changes_with_two_playing_decks() {
    let track_file = wav_file(4096, 4800);
    let path = track_file.path().canonicalize().unwrap();
    let mut phases = Vec::with_capacity(2);
    for active in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("gig-measurement.sqlite3");
        let (store, mut library) = open_gig_store(&db);
        assert_eq!(library.insert([path.clone()]), 1);
        let track = library.track_id_for_path(&path).unwrap();
        let gigs = [
            library.create_gig("First room").unwrap(),
            library.create_gig("Second room").unwrap(),
        ];
        save_changes(&mut library, &store);
        eprintln!("R02 gig changes and SQLite commits active={active}");
        let (cycles, result) = std::thread::scope(|scope| {
            let (finish, stop) = mpsc::channel::<()>();
            let (ready, started) = mpsc::sync_channel(1);
            let worker = scope.spawn(move || {
                let mut cycles = 0;
                ready.send(()).unwrap();
                while stop.recv_timeout(Duration::from_millis(100))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    if active {
                        cycles += 1;
                        gig_cycle(&mut library, &store, gigs, track, cycles);
                    }
                }
                store.close_delta(None).unwrap();
                cycles
            });
            started.recv_timeout(Duration::from_secs(10)).unwrap();
            let result = super::render_bench::measure_routed(30, 4, |_| {}, || {}, Some(false));
            drop(finish);
            (worker.join().unwrap(), result)
        });
        let (store, saved) = open_gig_store(&db);
        let history = saved.catalog().history;
        if active {
            assert!(cycles >= 200, "Only {cycles} gig changes in the 30 s run");
            let last = ((cycles - 1) % 2) as usize;
            assert_eq!(saved.active_gig(), Some(gigs[last]));
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].track_id, track);
            assert_eq!(u64::from(history[0].play_count), cycles);
            assert_eq!(history[0].last_played_ms, cycles);
            assert_eq!(
                saved.gig_rule(gigs[last], track).unwrap().fulfilled_at_ms,
                Some(cycles)
            );
            assert_eq!(
                saved.gig_rule(gigs[1 - last], track).unwrap().status,
                GigStatus::Avoid
            );
        } else {
            assert_eq!(cycles, 0);
            assert!(history.is_empty());
            assert_eq!(saved.active_gig(), Some(gigs[1]));
            assert!(gigs.iter().all(|&gig| saved.gig_rule(gig, track).is_none()));
        }
        store.close_delta(None).unwrap();
        eprintln!(
            "R02 completed and acknowledged gig changes={cycles}, persisted plays={}",
            history.first().map_or(0, |entry| entry.play_count)
        );
        phases.push(result);
    }
    assert_eq!(
        phases,
        [(0, 0, 0, 0); 2],
        "reference and load: render overruns, missed deadlines, bad signal blocks, allocations"
    );
}
