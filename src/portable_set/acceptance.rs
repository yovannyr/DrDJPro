use super::*;
use crate::test_support::wav_samples;

fn source(path: PathBuf, title: &str, position: u64) -> Source {
    Source {
        path,
        title: title.into(),
        in_ns: position,
        out_ns: Some(position + 1_000_000),
        cues: vec![Mark {
            slot: 2,
            ns: position,
            label: "bridge".into(),
            rgb: 0x336699,
        }],
        attach: false,
    }
}

#[test]
fn d08_offline_media_and_repeated_entries_survive_a_move_and_detect_single_byte_damage() {
    let dir = tempfile::tempdir().unwrap();
    let sources = dir.path().join("originals");
    let mut entries = Vec::new();
    for (i, sample) in [1100i16, -2000].into_iter().enumerate() {
        let folder = sources.join(i.to_string());
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("same.wav");
        fs::copy(wav_samples(&[sample; 400], 8000, 1).path(), &path).unwrap();
        entries.push(source(
            path,
            &format!("Track {i}"),
            (i as u64 + 1) * 10_000_000,
        ));
    }
    let mut repeated = entries[0].clone();
    repeated.in_ns = 5_000_000;
    repeated.out_ns = Some(9_000_000);
    entries.push(repeated);
    // A separate filesystem can be supplied for the native offline-transfer check.
    let volume = std::env::var_os("DRDJ_TEST_PACKAGE_DESTINATION");
    let package_dir = match volume {
        Some(path) => tempfile::tempdir_in(path),
        None => tempfile::tempdir(),
    }
    .unwrap();
    let dest = package_dir.path().join("Set");
    let cancel = Cancellation::default();
    export(&entries, "Gig", &dest, true, &cancel).unwrap();
    let published = fs::read(dest.join(MANIFEST)).unwrap();
    // Pin the same package on all three CI systems. The manifest itself fixes every
    // medium's SHA-256, which inspect and verify_paths independently recompute below.
    assert_eq!(
        format!("{:x}", Sha256::digest(&published)),
        "4b7398450b21d5e1aa1373b8dd2277cddcff7c65c672f3c59c58a5e4b5096468"
    );
    let next = package_dir.path().join("Second set");
    let err = export_with_media(&entries, "Gig", &next, true, &cancel, |_, file, _| {
        file.write_all(b"first block").unwrap();
        Err(std::io::Error::from(std::io::ErrorKind::StorageFull).to_string())
    })
    .unwrap_err();
    assert_eq!(
        err,
        std::io::Error::from(std::io::ErrorKind::StorageFull).to_string()
    );
    assert!(!next.exists());
    assert_eq!(fs::read_dir(package_dir.path()).unwrap().count(), 1);
    assert_eq!(fs::read(dest.join(MANIFEST)).unwrap(), published);
    fs::rename(&sources, dir.path().join("disconnected")).unwrap();
    let expected_manifest = fs::read(dest.join(MANIFEST)).unwrap();
    let set = inspect(&dest, |_| Ok(vec![]), &cancel).unwrap();
    assert!(!set.damaged);
    verify_paths(&set.set, &cancel).unwrap();
    let paths = &set.set.paths;
    assert!(paths.iter().all(Option::is_some));
    assert_eq!(paths[0], paths[2]);
    assert_ne!(paths[0], paths[1]);
    let actual: Vec<_> = set
        .set
        .manifest
        .entries
        .iter()
        .map(|e| (&e.title, e.in_ns, e.out_ns, e.cues[0].ns))
        .collect();
    let expected: Vec<_> = entries
        .iter()
        .map(|e| (&e.title, e.in_ns, e.out_ns, e.cues[0].ns))
        .collect();
    assert_eq!(actual, expected);
    let media = fs::read_dir(dest.join("media"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .collect::<Vec<_>>();
    assert_eq!(media.len(), 2);
    let size = media.iter().sum::<u64>() + expected_manifest.len() as u64;
    assert!(export(&entries, "overwrite", &dest, true, &cancel).is_err());
    assert_eq!(fs::read(dest.join(MANIFEST)).unwrap(), expected_manifest);
    let path = paths[0].as_ref().unwrap();
    let mut damaged = fs::read(path).unwrap();
    let last = damaged.last_mut().unwrap();
    *last ^= 1;
    fs::write(path, damaged).unwrap();
    let bad = inspect(&dest, |_| Ok(vec![]), &cancel).unwrap();
    assert!(
        bad.damaged
            && bad.set.paths[0].is_none()
            && bad.set.paths[1].is_some()
            && bad.set.paths[2].is_none()
    );
    assert!(verify_paths(&set.set, &cancel).is_err());
    let mut invalid = set.set.manifest;
    invalid.entries[0].media = Some("../outside.wav".into());
    fs::write(dest.join(MANIFEST), serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(inspect(&dest, |_| Ok(vec![]), &cancel).is_err());
    assert!(!package_dir.path().join("outside.wav").exists());
    println!(
        "D08: 3 entries, 2 same-name recordings, {size} package bytes, one changed byte detected"
    );
}

#[test]
fn m06_metadata_only_and_selective_media_match_hashes_in_a_different_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav_samples(&[1024; 800], 8000, 1);
    let b = wav_samples(&[2048; 800], 8000, 1);
    let c = wav_samples(&[-4096; 800], 8000, 1);
    let mut entries = [
        source(a.path().to_owned(), "A", 100),
        source(b.path().to_owned(), "B", 200),
        source(c.path().to_owned(), "C", 300),
    ];
    let catalog = dir.path().join("new-catalog");
    fs::create_dir(&catalog).unwrap();
    let paths = [
        catalog.join("renamed A.wav"),
        catalog.join("renamed B.wav"),
        catalog.join("same name, wrong edit.wav"),
    ];
    for (src, dst) in [
        a.path(),
        b.path(),
        wav_samples(&[-4095; 800], 8000, 1).path(),
    ]
    .iter()
    .zip(&paths)
    {
        fs::copy(src, dst).unwrap();
    }
    let dest = dir.path().join("handoff");
    let token = Cancellation::default();
    export(&entries, "Gig", &dest, false, &token).unwrap();
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
    let files = [
        (2, paths[1].clone()),
        (75, paths[2].clone()),
        (108, paths[0].clone()),
    ];
    let mut imported = inspect(
        &dest,
        |after| {
            Ok(files
                .iter()
                .filter(|(id, _)| *id > after)
                .cloned()
                .collect())
        },
        &token,
    )
    .unwrap();
    assert_eq!(
        imported.set.paths,
        [Some(paths[0].clone()), Some(paths[1].clone()), None]
    );
    assert!(resolve(&mut imported.set, 2, &paths[2], &token).is_err());
    assert_eq!(imported.set.manifest.entries[1].cues[0].ns, 200);
    let duplicate = catalog.join("copy.wav");
    fs::copy(&paths[0], &duplicate).unwrap();
    let matches = [(10, paths[0].clone()), (11, duplicate.clone())];
    let mut ambiguous = inspect(
        &dest,
        |after| {
            Ok(matches
                .iter()
                .filter(|(id, _)| *id > after)
                .cloned()
                .collect())
        },
        &token,
    )
    .unwrap();
    assert!(ambiguous.set.paths[0].is_none());
    resolve(&mut ambiguous.set, 0, &duplicate, &token).unwrap();
    assert_eq!(
        ambiguous.set.paths[0],
        Some(duplicate.canonicalize().unwrap())
    );
    entries[2].attach = true;
    let chosen = dir.path().join("with-clip");
    export(&entries, "Gig", &chosen, false, &token).unwrap();
    assert_eq!(fs::read_dir(chosen.join("media")).unwrap().count(), 1);
    let chosen_set = inspect(&chosen, |_| Ok(vec![]), &token).unwrap();
    assert!(
        chosen_set.set.paths[0].is_none()
            && chosen_set.set.paths[1].is_none()
            && chosen_set.set.paths[2].is_some()
    );
    token.request();
    let unfinished = dir.path().join("cancelled");
    assert!(export(&entries, "Gig", &unfinished, true, &token).is_err());
    assert!(!unfinished.exists());
}

#[test]
fn d08_cancel_during_real_media_copy_removes_the_partial_staging_directory() {
    struct CancelAfterWrite<'a> {
        file: &'a mut File,
        cancel: &'a Cancellation,
    }
    impl Write for CancelAfterWrite<'_> {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.file.write_all(data)?;
            self.cancel.request();
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }

    let volume = std::env::var_os("DRDJ_TEST_PACKAGE_DESTINATION");
    let dir = volume
        .map_or_else(tempfile::tempdir, tempfile::tempdir_in)
        .unwrap();
    let recording = wav_samples(&vec![42; 40_000], 8000, 1);
    let entries = [source(recording.path().to_owned(), "Cancel test", 0)];
    let cancel = Cancellation::default();
    let published = dir.path().join("Existing");
    export(&entries, "Existing", &published, true, &cancel).unwrap();
    let manifest = fs::read(published.join(MANIFEST)).unwrap();
    let source_bytes = fs::metadata(recording.path()).unwrap().len();
    let finished_bytes = manifest.len() as u64 + source_bytes;
    let partial_bytes = std::cell::Cell::new(0);
    let target = dir.path().join("Next");
    let result = export_with_media(
        &entries,
        "Next",
        &target,
        true,
        &cancel,
        |path, file, token| {
            let result = copy_hash(
                path,
                Some(&mut CancelAfterWrite {
                    file,
                    cancel: token,
                }),
                token,
            );
            partial_bytes.set(file.metadata().unwrap().len());
            result
        },
    );
    assert_eq!(result.unwrap_err(), cancel.check().unwrap_err());
    assert_eq!(partial_bytes.get(), 65_536);
    assert!(partial_bytes.get() < source_bytes);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(fs::read(published.join(MANIFEST)).unwrap(), manifest);
    let fresh = Cancellation::default();
    let restored = inspect(&published, |_| Ok(vec![]), &fresh).unwrap();
    verify_paths(&restored.set, &fresh).unwrap();
    eprintln!("D08 destination file bytes: existing={finished_bytes} additional_stage={} after_cleanup={finished_bytes}", partial_bytes.get());
}
