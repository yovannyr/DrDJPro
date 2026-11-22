//! Single-use loopback listener for the system browser. Never echo query parameters.
use super::{opaque, remaining, AtomicBool, Duration, Instant, TcpListener, DENIED, INVALID};
use std::{
    io::{ErrorKind, Read, Write},
    net::TcpStream,
};
use zeroize::Zeroizing;

type Code = (Zeroizing<String>, Option<String>);

enum Callback {
    Ignore,
    Denied,
    Accepted(Code),
}

fn parse(request: &[u8], host: &str, expected_state: &str) -> Callback {
    let Ok(request) = std::str::from_utf8(request) else {
        return Callback::Ignore;
    };
    let mut lines = request.split("\r\n");
    let mut words = lines.next().unwrap_or_default().split(' ');
    let (Some("GET"), Some(path), Some("HTTP/1.1"), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Callback::Ignore;
    };
    let mut hosts = lines
        .take_while(|s| !s.is_empty())
        .filter_map(|s| s.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("Host"));
    if hosts.next().is_none_or(|(_, value)| value.trim() != host) || hosts.next().is_some() {
        return Callback::Ignore;
    }
    let Some(query) = path.strip_prefix("/auth/callback?") else {
        return Callback::Ignore;
    };
    let (mut state, mut code, mut client, mut error) = (None, None, None, None);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let entry = match key.as_ref() {
            "state" => &mut state,
            "code" => &mut code,
            "client_id" => &mut client,
            "error" => &mut error,
            _ => continue,
        };
        if entry.replace(value).is_some() {
            return Callback::Ignore;
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Callback::Ignore;
    }
    if error.is_some() {
        return Callback::Denied;
    }
    match code {
        Some(value) if opaque(&value) => Callback::Accepted((
            Zeroizing::new(value.into_owned()),
            client.map(|v| v.into_owned()),
        )),
        _ => Callback::Ignore,
    }
}

fn handle(
    stream: &mut TcpStream,
    host: &str,
    state: &str,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Callback {
    let end = deadline.min(Instant::now() + Duration::from_secs(1));
    let mut request = Zeroizing::new([0; 8192]);
    let mut filled = 0;
    while filled < request.len() {
        let Ok(left) = remaining(cancel, end) else {
            break;
        };
        if stream
            .set_read_timeout(Some(left.min(Duration::from_millis(160))))
            .is_err()
        {
            break;
        }
        match stream.read(&mut request[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
        if request[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
            return parse(&request[..filled], host, state);
        }
    }
    Callback::Ignore
}

pub(super) fn receive(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Code, String> {
    let host = listener.local_addr().map_err(|_| INVALID)?.to_string();
    listener.set_nonblocking(true).map_err(|_| INVALID)?;
    loop {
        let left = remaining(cancel, deadline)?;
        let (mut stream, peer) = match listener.accept() {
            Ok(pair) => pair,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25).min(left));
                continue;
            }
            Err(_) => return Err(INVALID.into()),
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let _ = stream.set_nonblocking(false);
        let callback = handle(&mut stream, &host, state, cancel, deadline);
        let (status, text) = match &callback {
            Callback::Accepted(_) => ("200 OK", "Return to Dr. DJ Pro to finish signing in."),
            Callback::Denied => ("200 OK", "Authorization declined. Return to Dr. DJ Pro."),
            Callback::Ignore => ("400 Bad Request", "Invalid sign-in callback."),
        };
        let _ = stream.set_write_timeout(Some(Duration::from_millis(150)));
        let _ = write!(stream,"HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{text}",text.len());
        match callback {
            Callback::Accepted(code) => return Ok(code),
            Callback::Denied => return Err(DENIED.into()),
            Callback::Ignore => (),
        }
    }
}
