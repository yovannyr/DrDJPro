//! Real Settings page, provider worker, vault and HTTP adapter in an isolated process.
use drdjpro_ui_spike::{
    ai_registry::AiRegistry, ai_router::AiInput, ai_settings_ui, AiPage, AppWindow,
};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle, Model, Timer, TimerMode};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

fn control(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap_or_else(|| panic!("Missing {label}"))
}

fn click(ui: &AppWindow, label: &str) {
    control(ui, label).invoke_accessible_default_action();
}

fn wait_for_worker(ui: &AppWindow) {
    let window = ui.as_weak();
    let start = Instant::now();
    let timer = Timer::default();
    let quit_requested = std::cell::Cell::new(false);
    timer.start(TimerMode::Repeated, Duration::from_millis(5), move || {
        let finished = start.elapsed() > Duration::from_secs(12)
            || !window.unwrap().global::<AiPage>().get_working();
        if finished && !quit_requested.replace(true) {
            slint::quit_event_loop().unwrap();
        }
    });
    while ui.global::<AiPage>().get_working() && start.elapsed() <= Duration::from_secs(12) {
        slint::run_event_loop_until_quit().unwrap();
    }
    assert!(
        !ui.global::<AiPage>().get_working(),
        "worker did not finish"
    );
}

fn mock_provider() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let thread = thread::spawn(move || {
        for assistant in [false, true] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /v1/chat/completions "));
            let mut length = 0;
            let mut authenticated = false;
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((header, value)) = line.split_once(':') {
                    if header.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                    if header.eq_ignore_ascii_case("authorization") {
                        authenticated = value.trim() == "Bearer ui-test-credential";
                    }
                }
            }
            assert!(authenticated);
            assert!((1..8192).contains(&length));
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["model"], "test-model");
            let (mime, reply) = if assistant {
                assert_eq!(request["stream"], true);
                assert!(request["messages"].to_string().contains("Sample"));
                assert_eq!(
                    request["tools"][0]["function"]["name"],
                    "read_selected_track"
                );
                ("text/event-stream", "data: {\"choices\":[{\"delta\":{\"content\":\"Suggestion\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
            } else {
                (
                    "application/json",
                    r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}],"usage":{"total_tokens":9}}"#,
                )
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",reply.len()).unwrap();
        }
    });
    (url, thread)
}

#[test]
fn real_settings_adds_and_tests_api_provider_in_six_actions_and_clears_credentials() {
    // Scoped to this integration executable. No desktop account or persistent OS key is created.
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    i_slint_backend_testing::init_integration_test_with_system_time();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_path_buf();
    let ui_thread = thread::current().id();
    let ui = AppWindow::new().unwrap();
    ai_settings_ui::wire(
        &ui,
        || {
            Some(AiInput {
                title: "Sample".into(),
                artist: "Artist".into(),
            })
        },
        move || {
            assert_ne!(thread::current().id(), ui_thread);
            assert_eq!(thread::current().name(), Some("ai-provider-worker"));
            AiRegistry::open(&path)
        },
    );
    ui.window().set_size(slint::LogicalSize::new(850., 860.));
    ui.show().unwrap();
    let (url, server) = mock_provider();
    let start = Instant::now();
    click(&ui, "Open settings"); // 1
    click(&ui, "Settings: KI"); // 2
    wait_for_worker(&ui);
    let page = ui.global::<AiPage>();
    assert!(page.get_ready());
    assert!(page.get_local_only());
    for name in [
        "AI provider base URL",
        "AI default model",
        "New AI credential",
        "Save and test AI provider",
    ] {
        let item = control(&ui, name);
        let position = item.absolute_position();
        let size = item.size();
        assert!(
            position.x >= 0.
                && position.y >= 0.
                && position.x + size.width <= 850.
                && position.y + size.height <= 860.,
            "{name} outside the supported minimum window at {position:?}, {size:?}"
        );
    }
    control(&ui, "AI provider base URL").set_accessible_value(url); // 3
    control(&ui, "AI default model").set_accessible_value("test-model"); // 4
    control(&ui, "New AI credential").set_accessible_value("  Bearer ui-test-credential  "); // 5
    click(&ui, "Save and test AI provider"); // 6
    wait_for_worker(&ui);
    let status = page.get_status();
    assert!(
        status.starts_with("Connected (") && status.ends_with(" ms)"),
        "{status}"
    );
    assert!(start.elapsed() < Duration::from_secs(60));
    println!(
        "SC-001: six automated controls, {} ms including storage and HTTP probe",
        start.elapsed().as_millis()
    );
    let entry = page.get_entries().row_data(0).unwrap();
    assert_eq!(entry.status, status);
    assert!(entry.has_key);
    assert_eq!(entry.form.model, "test-model");
    assert!(entry.usage.contains("9"));
    assert!(entry.details.contains("lokal"));
    assert_eq!(page.get_selection(), 0);
    assert!(control(&ui, "New AI credential")
        .accessible_value()
        .unwrap_or_default()
        .is_empty());
    // Stored credentials never refill a reopened editor.
    click(&ui, "Back to decks");
    click(&ui, "Open settings");
    wait_for_worker(&ui);
    assert!(control(&ui, "New AI credential")
        .accessible_value()
        .unwrap_or_default()
        .is_empty());
    let db = rusqlite::Connection::open(directory.path().join("ai-providers.sqlite3")).unwrap();
    let preferences: String = db
        .query_row("SELECT payload FROM ai_preferences WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(preferences.contains("test-model") && !preferences.contains("ui-test-credential"));
    ui.set_selected_track(42);
    page.invoke_bind_feature(0, entry.form.id.clone(), entry.form.model.clone(), false);
    wait_for_worker(&ui);
    click(&ui, "Funktionen");
    assert_eq!(
        control(&ui, "Run AI feature on selected track").accessible_enabled(),
        Some(true)
    );
    click(&ui, "Run AI feature on selected track");
    wait_for_worker(&ui);
    assert_eq!(page.get_output(), "Suggestion", "{}", page.get_status());
    server.join().unwrap();
    click(&ui, "Zuordnung lösen");
    wait_for_worker(&ui);
    assert_eq!(
        control(&ui, "Run AI feature on selected track").accessible_enabled(),
        Some(false)
    );
    click(&ui, "Anbieter");
    control(&ui, "AI provider name").set_accessible_value("Unfinished edit");
    click(&ui, "Local AI only");
    wait_for_worker(&ui);
    assert!(!page.get_local_only());
    assert_eq!(
        control(&ui, "AI provider name").accessible_value().unwrap(),
        "Unfinished edit"
    );
    click(&ui, "Schlüssel dieses Anbieters entfernen");
    wait_for_worker(&ui);
    assert!(!page.get_entries().row_data(0).unwrap().has_key);
    let count: u32 = db
        .query_row("SELECT count(*) FROM credentials_v1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    click(&ui, "Entfernen");
    assert_eq!(page.get_entries().row_count(), 1);
    click(&ui, "Entfernen bestätigen");
    wait_for_worker(&ui);
    assert_eq!(page.get_entries().row_count(), 0);
}
