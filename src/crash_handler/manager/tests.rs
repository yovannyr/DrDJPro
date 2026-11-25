use super::*;

fn next(worker: &Worker) -> Event {
    worker.events.recv_timeout(Duration::from_secs(15)).unwrap()
}
fn request(worker: &Worker, id: u64, action: Action) -> Event {
    assert!(worker.send(Request { id, action }).is_ok());
    let reply = next(worker);
    assert_eq!(reply.id, id);
    reply
}

#[test]
fn startup_only_requests_previously_authorized_uploads_and_prompts_pending() {
    for (state, automatic, expected_upload, expected_prompt) in [
        (ReportStatus::Pending, false, false, true),
        (ReportStatus::Queued, false, true, false),
        (ReportStatus::NeedsConsent, true, false, true),
        (ReportStatus::Declined, true, false, false),
        (ReportStatus::Submitted, true, false, false),
        (ReportStatus::Pending, true, true, false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = CrashStore::open(temp.path().into()).unwrap();
        let report = store.insert("frame").unwrap();
        store
            .save_settings(&CrashSettings {
                auto_submit: automatic,
                server_url: "https://example.invalid/reports".into(),
            })
            .unwrap();
        store.set_status(&report.id, state).unwrap();
        let worker = Worker::spawn(store.clone()).unwrap();
        let event = next(&worker);
        assert_eq!(event.id, 0);
        assert_eq!(event.upload, expected_upload);
        assert_eq!(event.prompt, expected_prompt);
        assert_eq!(store.list().unwrap()[0].status, state); // Actor itself sends nothing.
    }
}

#[test]
fn explicit_actions_persist_consent_settings_and_delete_only_owned_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let store = CrashStore::open(temp.path().into()).unwrap();
    let report = store.insert("frame\n at /home/private/file.rs:4").unwrap();
    let dump = store.root().join(format!("{}.dmp", report.id));
    std::fs::write(&dump, b"local dump").unwrap();
    std::fs::write(store.root().join("unrelated.txt"), b"keep").unwrap();
    let worker = Worker::spawn(store.clone()).unwrap();
    let initial = next(&worker);
    assert!(initial.prompt);
    assert!(!initial.upload);
    assert!(!request(&worker, 1, Action::Send(report.id.clone(), String::new())).upload);
    assert_eq!(store.list().unwrap()[0].status, ReportStatus::Pending);
    let settings = CrashSettings {
        auto_submit: false,
        server_url: "https://example.invalid/reports".into(),
    };
    let saved = request(&worker, 2, Action::Save(settings.clone()));
    assert!(saved.settings_changed);
    let destination_id = saved.snapshot.unwrap().destination_id.unwrap();
    assert_eq!(store.settings().unwrap(), settings);
    assert!(request(&worker, 3, Action::Send(report.id.clone(), destination_id)).upload);
    assert_eq!(store.list().unwrap()[0].status, ReportStatus::Queued);
    assert!(!request(&worker, 4, Action::KeepLocal(report.id.clone())).upload);
    assert_eq!(store.list().unwrap()[0].status, ReportStatus::Declined);
    assert_eq!(std::fs::read(&dump).unwrap(), b"local dump");
    assert!(
        !request(
            &worker,
            5,
            Action::Save(CrashSettings {
                auto_submit: true,
                ..settings
            })
        )
        .upload
    );
    assert!(request(&worker, 6, Action::Delete(report.id.clone()))
        .snapshot
        .unwrap()
        .reports
        .is_empty());
    assert!(!dump.exists());
    assert_eq!(
        std::fs::read(store.root().join("unrelated.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn invalid_and_corrupt_settings_fail_closed_and_never_fake_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let store = CrashStore::open(temp.path().into()).unwrap();
    let report = store.insert("frame").unwrap();
    let settings = CrashSettings {
        server_url: "https://example.invalid/reports".into(),
        auto_submit: false,
    };
    store.save_settings(&settings).unwrap();
    let valid_id = store.sharing_settings().unwrap().1.unwrap();
    store.queue_report(&report.id).unwrap();
    std::fs::write(store.root().join("settings.json"), b"corrupt").unwrap();
    let worker = Worker::spawn(store.clone()).unwrap();
    let event = next(&worker);
    assert!(!event.upload);
    let data = event.snapshot.unwrap();
    assert!(data.destination_id.is_none());
    assert!(!data.settings.auto_submit);
    assert!(!data.settings_issue.is_empty());
    for (id, url) in [
        "",
        "http://localhost/",
        "https://user:pass@example.invalid/",
        "https://example.invalid/?secret=1",
    ]
    .into_iter()
    .enumerate()
    {
        let reply = request(
            &worker,
            id as u64 + 1,
            Action::Save(CrashSettings {
                auto_submit: true,
                server_url: url.into(),
            }),
        );
        assert!(!reply.upload);
        assert!(!reply.settings_changed);
        assert!(store.settings().is_err());
    }
    assert_eq!(store.list().unwrap()[0].status, ReportStatus::NeedsConsent);
    std::fs::write(store.root().join(format!("{}.json", report.id)), b"broken").unwrap();
    let failed = request(&worker, 9, Action::Send(report.id, valid_id));
    assert!(!failed.message.contains("no data was sent"));
    assert!(!failed.upload);
}

#[test]
fn stale_send_cannot_authorize_an_endpoint_changed_by_another_instance() {
    const ORIGINAL_URL: &str = "https://previous.invalid/reports";
    for return_to_original_url in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let store = CrashStore::open(temp.path().into()).unwrap();
        let mut settings = CrashSettings {
            server_url: ORIGINAL_URL.into(),
            auto_submit: true,
        };
        store.save_settings(&settings).unwrap();
        let report = store.insert("frame").unwrap();
        store.queue_report(&report.id).unwrap();
        let worker = Worker::spawn(store.clone()).unwrap();
        let shown = next(&worker).snapshot.unwrap().destination_id.unwrap();
        // The click predates the external change; the filesystem actor handles it afterwards.
        let old_click = Action::Send(report.id.clone(), shown.clone());
        let other = CrashStore::open(store.root().into()).unwrap();
        settings.server_url = "https://current.invalid/reports".into();
        other.save_settings(&settings).unwrap();
        if return_to_original_url {
            settings.server_url = ORIGINAL_URL.into();
            other.save_settings(&settings).unwrap();
        }

        let stale = request(&worker, 1, old_click);
        assert!(!stale.upload);
        let refreshed = stale.snapshot.unwrap();
        assert_eq!(refreshed.settings, settings);
        assert_eq!(refreshed.reports[0].status, ReportStatus::NeedsConsent);
        let current_id = refreshed.destination_id.unwrap();
        assert_ne!(current_id, shown);
        assert!(!request(&worker, 2, Action::Retry).upload);
        let fresh = request(&worker, 3, Action::Send(report.id, current_id));
        assert!(fresh.upload);
        assert_eq!(
            fresh.snapshot.unwrap().reports[0].status,
            ReportStatus::Queued
        );
    }
}

#[test]
fn closed_result_channel_does_not_leave_filesystem_worker_alive() {
    let temp = tempfile::tempdir().unwrap();
    let store = CrashStore::open(temp.path().into()).unwrap();
    let worker = Worker::spawn(store).unwrap();
    let sender = worker.commands.clone();
    drop(worker);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        match sender.try_send(Request {
            id: 1,
            action: Action::Retry,
        }) {
            Err(TrySendError::Disconnected(_)) => break,
            _ => {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
