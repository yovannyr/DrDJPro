//! Same-process, device-independent audio callback comparison while the real provider worker streams.
use drdjpro_ui_spike::ai_settings_ui::spawn_provider_worker;
use drdjpro_ui_spike::ai_transport::{execute, AiProtocol, AiRequest};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

struct Stop(Arc<AtomicBool>);
impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn stream_response(mut socket: TcpStream) -> std::io::Result<()> {
    // Winsock accepts a stream with the listener's nonblocking mode.
    socket.set_nonblocking(false)?;
    let short = Some(Duration::from_secs(2));
    socket.set_read_timeout(short)?;
    socket.set_write_timeout(short)?;
    socket.set_nodelay(true)?;
    let mut reader = BufReader::new(&mut socket);
    let mut len = 0;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.trim().strip_prefix("content-length: ") {
            len = value.parse().unwrap();
        }
    }
    let mut request = vec![0; len];
    reader.read_exact(&mut request)?;
    socket.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    )?;
    for _ in 0..16 {
        socket.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"beat \"}}]}\n\n")?;
        thread::sleep(Duration::from_millis(8));
    }
    socket.write_all(
        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
    )
}

#[test]
#[ignore = "opt-in equal-duration 10 ms playback comparison while the production AI worker streams"]
fn sc005_provider_load() {
    let seconds = std::env::var("DRDJ_AI_SOAK_SECONDS").map_or(30, |s| s.parse().unwrap());
    assert!((5..=600).contains(&seconds));
    let caller = thread::current().id();
    eprintln!("SC005 baseline:");
    let baseline = super::render_bench::measure(seconds, || {});
    let stop = Stop(Arc::new(AtomicBool::new(false)));
    let stop_server = Stop(Arc::new(AtomicBool::new(false)));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let flag = stop_server.0.clone();
    let mock = thread::spawn(move || {
        while !flag.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((socket, _)) => {
                    stream_response(socket).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("loopback server: {e}"),
            }
        }
    });
    let deltas = Arc::new(AtomicU64::new(0));
    let count = deltas.clone();
    let flag = stop.0.clone();
    let (ready, started) = mpsc::sync_channel(1);
    let provider = spawn_provider_worker(move || {
        let mut requests = 0;
        let worker = thread::current().id();
        assert_ne!(worker, caller);
        let req = AiRequest::Chat {
            model: "offline".into(),
            instruction: String::new(),
            input: "ping".into(),
            tools: vec![],
            max_tokens: 32,
            stream: true,
        };
        while !flag.load(Ordering::Acquire) {
            let reply = execute(
                AiProtocol::OpenAi,
                &base,
                None,
                &req,
                &AtomicBool::new(false),
                &mut |delta| {
                    assert_eq!(thread::current().id(), worker);
                    assert_eq!(delta, "beat ");
                    if count.fetch_add(1, Ordering::Release) == 0 {
                        ready.send(()).unwrap();
                    }
                },
            )
            .unwrap();
            assert_eq!(reply.text.len(), 16 * 5);
            requests += 1;
        }
        requests
    })
    .unwrap();
    let provider_id = provider.thread().id();
    started
        .recv_timeout(Duration::from_secs(5))
        .expect("first provider delta");
    let mut checkpoint = 0;
    eprintln!("SC005 loaded:");
    let start = Instant::now();
    let loaded = super::render_bench::measure(seconds, || {
        assert_ne!(thread::current().id(), provider_id);
        let now = deltas.load(Ordering::Acquire);
        assert!(
            now > checkpoint,
            "provider stopped delivering deltas during playback"
        );
        checkpoint = now;
    });
    drop(stop);
    let requests = provider.join().unwrap();
    drop(stop_server);
    mock.join().unwrap();
    eprintln!(
        "SC005 active {:.3}s, requests={requests}, deltas={}",
        start.elapsed().as_secs_f64(),
        deltas.load(Ordering::Relaxed)
    );
    assert!(requests > 1);
    assert_eq!(
        (baseline.0, baseline.2, baseline.3),
        (0, 0, 0),
        "baseline render overruns, gaps, allocations"
    );
    assert_eq!(
        (loaded.0, loaded.2, loaded.3),
        (0, 0, 0),
        "loaded render overruns, gaps, allocations"
    );
    assert!(
        loaded.1 <= baseline.1,
        "loaded deadline misses {} exceed baseline {}",
        loaded.1,
        baseline.1
    );
}
