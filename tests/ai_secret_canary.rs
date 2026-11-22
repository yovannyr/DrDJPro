//! Run the real capture and export paths after credential entry in a disposable
//! process. Only the parent knows the search marker; it reaches the child on stdin.
use drdjpro_ui_spike::{
    ai_registry::{AiPreset, AiRegistry},
    crash_handler::CrashStartup,
    sessions::mix_history_analytics::{ExportBuilder, QualityScore, SessionRecord},
};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;

#[test]
#[ignore = "subprocess helper for canary_stays_out_of_application_artifacts"]
fn canary_child() {
    let root = std::env::current_dir().unwrap();
    let crash = CrashStartup::at(
        Path::new(env!("CARGO_BIN_EXE_drdjpro-ui-spike")),
        &root.join("reports"),
    );
    assert!(crash.monitor_pid().is_some());
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let registry = AiRegistry::open(&root).unwrap();
    let mut secret = Zeroizing::new(String::new());
    std::io::stdin()
        .take(512)
        .read_to_string(&mut secret)
        .unwrap();
    assert!((32..128).contains(&secret.len()));
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut p = AiPreset::OpenAi.draft();
    p.base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    p.model = "canary-model".into();
    let id = registry.save(0, p, Some(&secret), false).unwrap();
    assert!(registry.has_credential(&id).unwrap());
    assert!(std::env::vars_os()
        .all(|(k, v)| !k.to_string_lossy().contains(&*secret)
            && !v.to_string_lossy().contains(&*secret)));
    assert!(std::env::args_os().all(|v| !v.to_string_lossy().contains(&*secret)));
    drop(secret);
    fs::write(
        root.join("preferences-export.json"),
        serde_json::to_vec(&registry.snapshot().unwrap()).unwrap(),
    )
    .unwrap();
    let session = SessionRecord::new(
        "sample-set",
        SystemTime::UNIX_EPOCH,
        Duration::from_secs(10),
        QualityScore::new(0.8),
        1,
    );
    for e in ExportBuilder::new(vec![session]).export_all() {
        e.save_to_file(&root.join("exports")).unwrap();
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let inflight = scope.spawn(|| registry.test(&id, &cancel));
        let (server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        // Leave the authentication header inside the kernel receive buffer
        // throughout a snapshot of the process doing the authenticated request.
        assert_eq!(server.peek(&mut [0u8; 1]).unwrap(), 1);
        assert!(scope
            .spawn(|| panic!("Intentional isolated diagnostics test"))
            .join()
            .is_err());
        cancel.store(true, std::sync::atomic::Ordering::Release);
        drop(server);
        assert!(inflight.join().unwrap().is_err());
    });
    assert!(!crash.store().unwrap().list().unwrap().is_empty());
    drop(crash);
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            collect(&p, files)
        } else {
            files.push(p)
        }
    }
}

#[test]
fn canary_stays_out_of_application_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let marker = format!("provider-test-{:032x}", u128::from_be_bytes(random));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .current_dir(temp.path())
        .args(["--exact", "canary_child", "--ignored", "--nocapture"])
        .stdin(Stdio::piped())
        .stdout(fs::File::create(temp.path().join("stdout.log")).unwrap())
        .stderr(fs::File::create(temp.path().join("stderr.log")).unwrap())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(marker.as_bytes())
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "isolated canary process failed");
            break;
        }
        if start.elapsed() > Duration::from_secs(40) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated canary process timed out")
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut files = Vec::new();
    collect(temp.path(), &mut files);
    for expected in [
        "ai-providers.sqlite3",
        "preferences-export.json",
        "mix_history_export.json",
    ] {
        assert!(
            files
                .iter()
                .any(|p| p.file_name().is_some_and(|s| s == expected)),
            "missing {expected}"
        )
    }
    assert!(
        files
            .iter()
            .any(|p| p.extension().is_some_and(|v| v == "dmp")),
        "missing native snapshot"
    );
    assert!(
        files
            .iter()
            .any(|p| p.file_name().is_some_and(|v| v == "runtime.jsonl")),
        "missing runtime log"
    );
    let wide: Vec<u8> = marker.encode_utf16().flat_map(u16::to_le_bytes).collect();
    for path in files {
        let bytes = fs::read(&path).unwrap();
        for needle in [marker.as_bytes(), wide.as_slice()] {
            assert!(
                !bytes.windows(needle.len()).any(|b| b == needle),
                "test credential appeared in {}",
                path.file_name().unwrap().to_string_lossy()
            );
        }
    }
}
