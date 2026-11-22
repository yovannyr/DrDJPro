use super::*;
use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Arc},
    thread,
};

fn receive_request(listener: TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + TEST_TIME;
    let mut socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("Worker did not connect: {e}"),
        }
    };
    socket.set_nonblocking(false).unwrap();
    socket.set_read_timeout(Some(TEST_TIME)).unwrap();
    let mut reader = BufReader::new(&mut socket);
    let mut size = 0;
    let mut line = String::new();
    loop {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                size = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; size];
    reader.read_exact(&mut body).unwrap();
    socket
}

fn check_cancel(streaming: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (fragment, received_fragment) = mpsc::channel();
    let (done, received_done) = mpsc::channel();
    let (resume, next_url) = mpsc::channel::<String>();
    let worker = thread::spawn(move || {
        let request = if streaming {
            AiRequest::Chat {
                model: "fixture".into(),
                instruction: String::new(),
                input: "test".into(),
                tools: vec![],
                max_tokens: 10,
                stream: true,
            }
        } else {
            AiRequest::Models
        };
        let result = execute(
            AiProtocol::OpenAi,
            &url,
            None,
            &request,
            &worker_cancel,
            &mut |text| {
                fragment.send(text.to_owned()).unwrap();
            },
        );
        done.send(result.err()).unwrap();
        let url = next_url.recv_timeout(TEST_TIME).unwrap();
        execute(
            AiProtocol::OpenAi,
            &url,
            None,
            &AiRequest::Models,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap()
        .models
    });

    let mut socket = receive_request(listener);
    if streaming {
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"start\"}}]}\n\n").unwrap();
        assert_eq!(received_fragment.recv_timeout(TEST_TIME).unwrap(), "start");
    }
    // Let the worker settle into the next socket wait after delivering the
    // fragment. A cancel racing the callback alone would miss a blocking read.
    assert_eq!(
        received_done.recv_timeout(Duration::from_millis(75)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    let start = Instant::now();
    cancel.store(true, Ordering::Release);
    let tolerance = Duration::from_millis(500);
    assert_eq!(
        received_done.recv_timeout(tolerance).unwrap(),
        Some(AiError::Cancelled)
    );
    // The worker is idle until we send the next URL. The peer must see the
    // aborted connection close even if another provider request never arrives.
    socket.set_read_timeout(Some(tolerance)).unwrap();
    match socket.read(&mut [0]) {
        Ok(0) => {}
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("Aborted HTTP connection still active: {result:?}"),
    }
    eprintln!(
        "Stalled {} connection closed after {:?}",
        if streaming { "body" } else { "headers" },
        start.elapsed()
    );
    let (url, second_server) =
        tests::server(vec![tests::http(200, "", r#"{"data":[{"id":"ok"}]}"#)]);
    resume.send(url).unwrap();
    assert_eq!(worker.join().unwrap(), ["ok"]);
    assert_eq!(second_server.join().unwrap().len(), 1);
}

#[test]
fn cancel_stalled_headers_closes_socket_and_releases_worker() {
    check_cancel(false);
}

#[test]
fn cancel_stalled_stream_closes_socket_and_releases_worker() {
    check_cancel(true);
}
