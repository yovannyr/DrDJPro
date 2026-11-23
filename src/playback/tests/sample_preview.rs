use super::*;
use crate::cancellation::Cancellation;
use std::sync::mpsc;

fn frame(output: &mut impl Iterator<Item = f32>) -> [f32; 4] {
    std::array::from_fn(|_| output.next().unwrap())
}

#[test]
fn m01_twenty_one_shots_switch_only_the_headphone_pair_without_render_allocations() {
    const RATE: u32 = 48_000;
    let deck = wav_file(4096, RATE);
    let make = || {
        let (mixer, output) = rodio::mixer::mixer(4, RATE);
        let engine = DualDeckEngine::from_mixer(mixer, 4, RATE);
        for id in [DeckId::A, DeckId::B] {
            engine
                .try_install(id, PreparedTrack::open(deck.path()).unwrap())
                .unwrap();
            engine.play(id);
        }
        (engine, output)
    };
    let (engine, mut output) = make();
    let (_control, mut baseline) = make();
    for _ in 0..480 {
        assert_eq!(frame(&mut output), frame(&mut baseline));
    }
    let cancel = Cancellation::default();
    for index in 0..20 {
        let value = 512 + index * 160;
        let file = wav_file(value, 512);
        let clip = PreviewClip::open_sample(file.path(), Duration::ZERO, &cancel).unwrap();
        engine.play_sample_preview(&clip).unwrap();
        let expected = f32::from(value) / 32768. * 0.7;
        let count = if index == 19 { 520 } else { 64 };
        let heap = crate::test_alloc::measured_heap(|| {
            for i in 0..count {
                let actual = frame(&mut output);
                let normal = frame(&mut baseline);
                assert_eq!(actual[..2], normal[..2]);
                assert!(normal[0] > 0.);
                let expected = if i < 512 { expected } else { 0. };
                assert!((actual[2] - expected).abs() < 1e-6 && actual[2] == actual[3]);
            }
        });
        assert_eq!(
            (heap.allocations, heap.reallocations, heap.deallocations),
            (0, 0, 0)
        );
        if index == 19 {
            engine.play_sample_preview(&clip).unwrap();
            assert!(frame(&mut output)[2] > 0.);
            frame(&mut baseline);
            engine.stop_tempo_preview();
            assert_eq!(frame(&mut output), frame(&mut baseline));
            engine.set_output_pair(1).unwrap();
            assert!(engine.play_sample_preview(&clip).is_err());
            let (stereo, mut silent) = rodio::mixer::mixer(2, RATE);
            let stereo = DualDeckEngine::from_mixer(stereo, 2, RATE);
            assert!(stereo.play_sample_preview(&clip).is_err());
            assert!(silent.by_ref().take(512).all(|s| s == 0.));
        }
    }
}

#[test]
fn m01_preview_checks_file_revision_at_replay_and_decodes_only_three_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.wav");
    let original = wav_file(4096, 8_000 * 24);
    let replacement = wav_file(-4096, 8_000 * 24);
    std::fs::copy(original.path(), &path).unwrap();
    let cancel = Cancellation::default();
    let clip = PreviewClip::open_sample(&path, Duration::ZERO, &cancel).unwrap();
    assert_eq!(clip.pcm.len(), 48_000 * 3);
    let stored =
        PreviewClip::open_sample_or_cached(&path, Duration::ZERO, &cancel, Some(clip.clone()))
            .unwrap();
    assert!(Arc::ptr_eq(&clip.pcm, &stored.pcm));
    let modified = path.metadata().unwrap().modified().unwrap();
    std::fs::copy(replacement.path(), &path).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified + Duration::from_secs(10))
        .unwrap();
    let fresh =
        PreviewClip::open_sample_or_cached(&path, Duration::ZERO, &cancel, Some(clip.clone()))
            .unwrap();
    assert!(fresh.pcm[1] < 0. && !Arc::ptr_eq(&clip.pcm, &fresh.pcm));
    let shifted = PreviewClip::open_sample_or_cached(
        &path,
        Duration::from_secs(3),
        &cancel,
        Some(fresh.clone()),
    )
    .unwrap();
    assert_eq!(shifted.pcm.len(), 48_000);
    std::fs::remove_file(&path).unwrap();
    assert!(
        PreviewClip::open_sample_or_cached(&path, Duration::ZERO, &cancel, Some(fresh)).is_err()
    );
    cancel.request();
    assert!(PreviewClip::open_sample(original.path(), Duration::ZERO, &cancel).is_err());
}

#[test]
fn m01_preview_rejects_replaced_and_edited_samples_with_preserved_length_and_mtime() {
    use std::{fs, io::Write};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audition.wav");
    let replacement = dir.path().join("replacement.wav");
    let positive = wav_file(4096, 48_000);
    let negative = wav_file(-4096, 48_000);
    fs::copy(positive.path(), &path).unwrap();
    fs::copy(negative.path(), &replacement).unwrap();
    let mtime = path.metadata().unwrap().modified().unwrap();
    let writable = |p: &Path| fs::OpenOptions::new().write(true).open(p).unwrap();
    writable(&replacement).set_modified(mtime).unwrap();
    assert_eq!(
        replacement.metadata().unwrap().len(),
        path.metadata().unwrap().len()
    );
    assert_eq!(replacement.metadata().unwrap().modified().unwrap(), mtime);
    let cancel = Cancellation::default();
    let initial = PreviewClip::open_sample(&path, Duration::ZERO, &cancel).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert_eq!(path.metadata().unwrap().modified().unwrap(), mtime);
    let replaced =
        PreviewClip::open_sample_or_cached(&path, Duration::ZERO, &cancel, Some(initial)).unwrap();
    assert!(
        replaced.pcm[1] < 0.,
        "replacing the directory entry must invalidate the original file ID"
    );

    // Give even one-second change-time clocks a new tick, without using a change in mtime as evidence.
    std::thread::sleep(Duration::from_millis(1100));
    {
        let mut file = writable(&path);
        file.write_all(&fs::read(positive.path()).unwrap()).unwrap();
    }
    writable(&path).set_modified(mtime).unwrap();
    assert_eq!(path.metadata().unwrap().modified().unwrap(), mtime);
    let edited =
        PreviewClip::open_sample_or_cached(&path, Duration::ZERO, &cancel, Some(replaced)).unwrap();
    assert!(
        edited.pcm[1] > 0.,
        "an in-place rewrite must invalidate the previous change time"
    );
}

#[cfg(unix)]
#[test]
fn m01_preview_rejects_a_replacement_fifo_without_waiting_for_a_writer() {
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.wav");
    let wav = wav_file(4096, 512);
    std::fs::copy(wav.path(), &path).unwrap();
    let cached = PreviewClip::open_sample(&path, Duration::ZERO, &Cancellation::default()).unwrap();
    std::fs::remove_file(&path).unwrap();
    let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: fifo is a live NUL-terminated path and 0600 is a valid mode.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let selected = &path;
        scope.spawn(move || {
            let result = PreviewClip::open_sample_or_cached(
                selected,
                Duration::ZERO,
                &Cancellation::default(),
                Some(cached),
            );
            let _ = tx.send(result.err());
        });
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Some(error)) => assert!(error.contains("regular audio file"), "{error}"),
            result => {
                // Release a blocked open and deliver EOF before the scope joins a regressed worker.
                drop(
                    std::fs::File::options()
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&path),
                );
                panic!("Expected a prompt FIFO rejection: {result:?}");
            }
        }
    });
}

#[test]
fn m01_preview_cache_charges_allocated_capacity_and_evicts_least_recently_used() {
    let file = wav_file(4096, 16);
    let seed =
        PreviewClip::open_sample(file.path(), Duration::ZERO, &Cancellation::default()).unwrap();
    let make = |bytes| {
        let mut clip = seed.clone();
        let mut samples = Vec::with_capacity(bytes / std::mem::size_of::<f32>());
        samples.push(0.);
        clip.pcm = Arc::new(samples);
        clip
    };
    let mut cache = SamplePreviewCache::default();
    let size = MAX_SAMPLE_PREVIEW_BYTES / 4 + 4;
    for name in ["A", "B", "C"] {
        cache.insert(name.into(), make(size));
    }
    assert_eq!(cache.bytes(), 3 * size);
    assert!(cache.get(Path::new("A")).is_some());
    cache.insert("D".into(), make(size));
    assert!(cache.get(Path::new("B")).is_none());
    assert_eq!(cache.bytes(), 3 * size);
    cache.insert("A".into(), make(MAX_SAMPLE_PREVIEW_BYTES + 4));
    assert!(cache.get(Path::new("A")).is_none());
    assert_eq!(cache.bytes(), 2 * size);
    cache.insert("D".into(), seed.clone());
    assert_eq!(cache.bytes(), size + seed.pcm_bytes());
    cache.clear();
    assert_eq!(cache.bytes(), 0);
}

#[test]
#[ignore = "two 30 s measurements with four-channel audio and local sample switches"]
fn m01_equal_duration_twenty_sample_auditions_with_two_decks() {
    const RATE: u32 = 48_000;
    let files: Vec<_> = (0..20)
        .map(|i| wav_file(1000 + 110 * i, RATE * 2))
        .collect();
    let paths: Vec<_> = files.iter().map(|file| file.path().to_owned()).collect();
    for active in [false, true] {
        let paths = &paths;
        eprintln!("M01 two-deck sample browser active={active}");
        let (audio, switches, hits, peak) = std::thread::scope(|scope| {
            let (send_control, receive_control) =
                mpsc::sync_channel::<Arc<crate::playback::tempo_preview::PreviewControl>>(1);
            let (finish, stop) = mpsc::channel::<()>();
            let worker = scope.spawn(move || {
                let Ok(control) = receive_control.recv() else {
                    return (0, 0, 0);
                };
                let cancel = Cancellation::default();
                let mut cache = SamplePreviewCache::default();
                let (mut switches, mut hits, mut peak) = (0, 0, 0);
                while stop.recv_timeout(Duration::from_millis(250))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    let path = &paths[switches % paths.len()];
                    let cached = cache.get(path);
                    let previous = cached.as_ref().map(|c| Arc::clone(&c.pcm));
                    let clip =
                        PreviewClip::open_sample_or_cached(path, Duration::ZERO, &cancel, cached)
                            .unwrap();
                    hits += usize::from(previous.is_some_and(|pcm| Arc::ptr_eq(&pcm, &clip.pcm)));
                    let voice = crate::playback::tempo_preview::Excerpt::new(&clip, RATE).unwrap();
                    cache.insert(path.clone(), clip);
                    peak = peak.max(cache.bytes());
                    assert!(peak <= MAX_SAMPLE_PREVIEW_BYTES);
                    control.start_sample(voice);
                    switches += 1;
                }
                (switches, hits, peak)
            });
            let audio = super::render_bench::measure_routed(
                30,
                4,
                move |engine| {
                    if active {
                        let clip = PreviewClip::open_sample(
                            &paths[0],
                            Duration::ZERO,
                            &Cancellation::default(),
                        )
                        .unwrap();
                        engine.play_sample_preview(&clip).unwrap();
                        send_control.send(engine.tempo_preview.clone()).unwrap();
                    }
                },
                || {},
                Some(active),
            );
            drop(finish);
            let (switches, hits, peak) = worker.join().unwrap();
            (audio, switches, hits, peak)
        });
        eprintln!("M01 sample switches={switches} verified_cache_hits={hits} peak_cached_pcm_bytes={peak} limit={MAX_SAMPLE_PREVIEW_BYTES}");
        if active {
            assert!(switches >= 90 && hits == switches - paths.len());
            assert!(peak > 0 && peak <= MAX_SAMPLE_PREVIEW_BYTES);
        } else {
            assert_eq!((switches, hits, peak), (0, 0, 0));
        }
        assert_eq!(
            audio,
            (0, 0, 0, 0),
            "render overruns, missed deadlines, bad signal blocks, allocations"
        );
    }
}
