use super::*;
use rsa::{rand_core::OsRng, traits::PublicKeyParts, Pkcs1v15Sign, RsaPrivateKey};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    sync::OnceLock,
    thread,
};

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        // Only signed mock replies and their public key outlive this setup.
        let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"fixture"}"#);
        let sign = |claims: &Value| {
            let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
            let input = format!("{header}.{encoded}");
            let signature = key
                .sign(
                    Pkcs1v15Sign::new::<Sha256>(),
                    &Sha256::digest(input.as_bytes()),
                )
                .unwrap();
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
        };
        let issued = now();
        let claims = json!({
            "iss": ISSUER, "aud": "fixture-client", "sub": "fixture-subject",
            "iat": issued, "exp": issued + 3600, "nonce": "fixture-nonce"
        });
        let mut missing_nonce = claims.clone();
        missing_nonce.as_object_mut().unwrap().remove("nonce");
        let mut multiple_audiences = claims.clone();
        multiple_audiences["aud"] = json!(["fixture-client", "other-client"]);
        json!({
            "jwks": {"keys": [{"kid":"fixture","kty":"RSA","alg":"RS256","use":"sig",
                "n":URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
                "e":URL_SAFE_NO_PAD.encode(key.e().to_bytes_be())}]},
            "id": sign(&claims), "missing_nonce": sign(&missing_nonce),
            "multi_without_azp": sign(&multiple_audiences)
        })
    })
}

#[derive(Default)]
struct Received {
    method: String,
    path: String,
    bearer: String,
    form: BTreeMap<String, String>,
}

fn read_http(stream: &TcpStream) -> Received {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let mut parts = line.split_ascii_whitespace();
    let (method, path) = (parts.next().unwrap().into(), parts.next().unwrap().into());
    let mut length = 0;
    let mut bearer = String::new();
    loop {
        line.clear();
        assert_ne!(r.read_line(&mut line).unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                bearer = value.trim().to_string();
            }
        }
    }
    let mut body = vec![0; length];
    r.read_exact(&mut body).unwrap();
    let form = form_urlencoded::parse(&body).into_owned().collect();
    Received {
        method,
        path,
        bearer,
        form,
    }
}

type Mock = (Provider, thread::JoinHandle<Vec<Received>>);
fn server(replies: impl FnOnce(&str) -> Vec<(u16, Value)>) -> Mock {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let replies = replies(&base);
    let worker = thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, body) in replies {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(4))
                    }
                    _ => panic!("Missing mock request"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            seen.push(read_http(&stream));
            let body = if body.is_null() {
                String::new()
            } else {
                body.to_string()
            };
            write!(stream,"HTTP/1.1 {status} Mock\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
        }
        seen
    });
    (Provider::open(&base).unwrap(), worker)
}

fn send_callback(redirect: &str, path: &str, host: Option<&str>) {
    let addr = Url::parse(redirect).unwrap().socket_addrs(|| None).unwrap()[0];
    let mut stream = TcpStream::connect(addr).unwrap();
    let expected_host = addr.to_string();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}\r\n\r\n",
        host.unwrap_or(&expected_host)
    )
    .unwrap();
}

#[test]
fn dynamic_login_then_refresh_and_revoke_with_a_mock_issuer() {
    let fixture = fixture();
    let (provider, worker) = server(|base| {
        vec![
            (
                200,
                json!({"access_token":"first","refresh_token":"refresh-one","id_token":fixture["id"],
            "token_type":"Bearer","expires_in":3600,"scope":PLAN_SCOPE}),
            ),
            (200, fixture["jwks"].clone()),
            (
                200,
                json!({"access_token":"second","refresh_token":"refresh-two","token_type":"bearer","expires_in":30}),
            ),
            (
                200,
                json!({"access_token":"third","token_type":"Bearer","expires_in":3600,"scope":"openid"}),
            ),
            (
                200,
                json!({"issuer":ISSUER,"revocation_endpoint":format!("{base}/revoke")}),
            ),
            (503, Value::Null),
            (200, Value::Null),
        ]
    });
    let cancel = AtomicBool::new(false);
    let mut auth = BTreeMap::<String, String>::new();
    let (account, first) = provider
        .login(
            "urn:uuid:fixture",
            None,
            &cancel,
            "fixture-nonce",
            || Ok(()),
            |u| {
                auth = u.query_pairs().into_owned().collect();
                let redirect = &auth["redirect_uri"];
                send_callback(redirect, "/favicon.ico", None);
                send_callback(redirect, "/auth/callback?state=wrong&code=bogus", None);
                let path = format!(
                    "/auth/callback?state={}&code=correct-code&client_id=fixture-client",
                    auth["state"]
                );
                send_callback(redirect, &path, Some("wrong.example"));
                send_callback(redirect, &path, None);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(account.client, "fixture-client");
    assert_eq!(account.subject, "fixture-subject");
    assert!(first.plan_enabled());
    assert!(!first.needs_refresh());
    let deadline = Instant::now() + Duration::from_secs(5);
    let second = provider.renew(&account, &first, &cancel, deadline).unwrap();
    assert!(second.needs_refresh());
    assert!(second.plan_enabled());
    let third = provider
        .renew(&account, &second, &cancel, deadline)
        .unwrap();
    assert!(!third.plan_enabled());
    provider.end(&account, &third).unwrap();
    let req = worker.join().unwrap();
    assert_eq!(auth["resource"], RESOURCE);
    assert_eq!(auth["client_id"], DYNAMIC);
    assert_eq!(auth["agent_name_hint"], "Dr. DJ Pro");
    assert_eq!(auth["code_challenge_method"], "S256");
    assert_eq!(auth["scope"], SCOPES);
    assert_eq!(req[0].method, "POST");
    assert_eq!(req[0].path, "/api/accounts/oauth/token");
    assert_eq!(req[0].form["resource"], RESOURCE);
    assert_eq!(req[0].form["client_id"], "fixture-client");
    assert_eq!(req[0].form["redirect_uri"], auth["redirect_uri"]);
    assert_eq!(req[0].form["code"], "correct-code");
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(req[0].form["code_verifier"].as_bytes())),
        auth["code_challenge"]
    );
    assert!(!auth.contains_key("code_verifier"));
    assert_eq!(req[1].path, "/.well-known/jwks.json");
    assert_eq!(req[2].form["refresh_token"], "refresh-one");
    assert!(!req[2].form.contains_key("scope"));
    assert_eq!(req[3].form["refresh_token"], "refresh-two");
    for r in &req[5..] {
        assert_eq!(r.path, "/revoke");
        assert_eq!(r.form["token"], "refresh-two");
        assert_eq!(r.form["token_type_hint"], "refresh_token");
    }
}

#[test]
fn saved_registration_rejects_wrong_client_or_identity() {
    let fixture = fixture();
    let (provider, worker) = server(|_| {
        vec![
            (
                200,
                json!({"access_token":"a","refresh_token":"r","expires_in":3600,"token_type":"Bearer","id_token":fixture["id"]}),
            ),
            (200, fixture["jwks"].clone()),
        ]
    });
    let old = Account {
        client: "fixture-client".into(),
        subject: "other-subject".into(),
    };
    let cancel = AtomicBool::new(false);
    for suffix in ["&client_id=other", ""] {
        assert!(provider
            .login(
                "urn:uuid:test",
                Some(&old),
                &cancel,
                "fixture-nonce",
                || Ok(()),
                |u| {
                    let params: BTreeMap<_, _> = u.query_pairs().collect();
                    assert_eq!(params["client_id"], old.client);
                    assert!(!params.contains_key("agent_name_hint"));
                    send_callback(
                        &params["redirect_uri"],
                        &format!("/auth/callback?state={}&code=a{suffix}", params["state"]),
                        None,
                    );
                    Ok(())
                }
            )
            .is_err());
    }
    assert_eq!(worker.join().unwrap().len(), 2);
}

#[test]
fn id_signature_nonce_audience_expiry_and_algorithm_are_required() {
    let f = fixture();
    let jwks = serde_json::from_value(f["jwks"].clone()).unwrap();
    let expected = || identity::Expected {
        client: "fixture-client",
        nonce: "fixture-nonce",
        refresh: false,
        access_token: "test",
        now: now(),
    };
    let id = f["id"].as_str().unwrap();
    assert_eq!(
        identity::subject(id, &jwks, expected()).unwrap(),
        "fixture-subject"
    );
    assert!(identity::subject(
        id,
        &jwks,
        identity::Expected {
            nonce: "wrong",
            ..expected()
        }
    )
    .is_err());
    assert!(identity::subject(
        id,
        &jwks,
        identity::Expected {
            client: "wrong",
            ..expected()
        }
    )
    .is_err());
    assert!(identity::subject(
        id,
        &jwks,
        identity::Expected {
            now: u64::MAX,
            ..expected()
        }
    )
    .is_err());
    assert!(identity::subject(f["missing_nonce"].as_str().unwrap(), &jwks, expected()).is_err());
    assert!(
        identity::subject(f["multi_without_azp"].as_str().unwrap(), &jwks, expected()).is_err()
    );
    let (head, rest) = id.split_once('.').unwrap();
    let (body, sig) = rest.split_once('.').unwrap();
    assert!(identity::subject(
        &format!("{head}.{body}.{}", URL_SAFE_NO_PAD.encode([0; 256])),
        &jwks,
        expected()
    )
    .is_err());
    let none = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","kid":"fixture"}"#);
    assert!(identity::subject(&format!("{none}.{body}.{sig}"), &jwks, expected()).is_err());
}

#[test]
fn cancel_timeout_and_duplicate_state_never_exchange_a_code() {
    let (provider, worker) = server(|_| vec![]);
    let cancel = AtomicBool::new(true);
    assert!(provider
        .login(
            "host",
            None,
            &cancel,
            "nonce",
            || Ok(()),
            |_| panic!("cancelled before browser")
        )
        .is_err());
    cancel.store(false, Ordering::Relaxed);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let redirect = format!("http://{}/auth/callback", listener.local_addr().unwrap());
    send_callback(
        &redirect,
        "/auth/callback?state=known&state=known&code=a",
        None,
    );
    assert!(callback::receive(
        &listener,
        "known",
        &cancel,
        Instant::now() + Duration::from_millis(80)
    )
    .is_err());
    send_callback(
        &redirect,
        "/auth/callback?state=known&error=access_denied",
        None,
    );
    assert_eq!(
        callback::receive(
            &listener,
            "known",
            &cancel,
            Instant::now() + Duration::from_secs(1)
        )
        .unwrap_err(),
        DENIED
    );
    worker.join().unwrap();
}

#[test]
fn cross_origin_revocation_is_rejected_before_sending_secrets() {
    let (provider, worker) = server(|_| {
        vec![(
            200,
            json!({"issuer":ISSUER,"revocation_endpoint":"https://example.org/revoke"}),
        )]
    });
    let account = Account {
        client: "fixture-client".into(),
        subject: "s".into(),
    };
    let tokens = Tokens {
        access_token: "a".into(),
        refresh_token: "secret".into(),
        scope: PLAN_SCOPE.into(),
        expires_at: 0,
        nonce: "n".into(),
    };
    assert!(provider.end(&account, &tokens).is_err());
    assert_eq!(worker.join().unwrap().len(), 1);
}

#[test]
fn inference_retry_uses_the_access_token_from_the_refresh_response() {
    let (provider, worker) = server(|_| {
        vec![
            (401, json!({"error":{"code":"invalid_token"}})),
            (
                200,
                json!({"access_token":"fresh-canary","refresh_token":"rotated","expires_in":3600,"token_type":"Bearer"}),
            ),
            (200, json!({"output_text":"ok"})),
        ]
    });
    let previous = Tokens {
        access_token: "stale-canary".into(),
        refresh_token: "old-refresh".into(),
        scope: PLAN_SCOPE.into(),
        expires_at: 0,
        nonce: "n".into(),
    };
    let account = Account {
        client: "fixture-client".into(),
        subject: "fixture-subject".into(),
    };
    let cancel = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(3);
    let inference = |tokens: &Tokens| {
        provider
            .http
            .post(provider.endpoint("/v1/responses").unwrap())
            .bearer_auth(&tokens.access_token)
            .json(&json!({"model":"mock","input":"ping"}))
    };
    assert_eq!(
        send(inference(&previous), &cancel, deadline)
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let fresh = provider
        .renew(&account, &previous, &cancel, deadline)
        .unwrap();
    let reply: Value = provider.json(inference(&fresh), &cancel, deadline).unwrap();
    assert_eq!(reply["output_text"], "ok");
    let seen = worker.join().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].bearer, "Bearer stale-canary");
    assert_eq!(seen[0].path, "/v1/responses");
    assert_eq!(seen[1].form["refresh_token"], "old-refresh");
    assert_eq!(seen[1].path, "/api/accounts/oauth/token");
    assert_eq!(seen[2].bearer, "Bearer fresh-canary");
    assert_eq!(seen[2].path, seen[0].path);
}

#[test]
fn limited_consent_keeps_identity_and_refresh_checks_replacement_identity() {
    let f = fixture();
    let (provider, worker) = server(|_| {
        vec![
            (
                200,
                json!({"access_token":"identity-access","refresh_token":"old","expires_in":3600,"token_type":"Bearer","id_token":f["id"],"scope":"openid profile"}),
            ),
            (200, f["jwks"].clone()),
            (
                200,
                json!({"access_token":"swapped","refresh_token":"rotated","expires_in":3600,"token_type":"Bearer","id_token":f["id"]}),
            ),
            (200, f["jwks"].clone()),
        ]
    });
    let cancel = AtomicBool::new(false);
    let (mut account, saved) = provider
        .login(
            "urn:uuid:test",
            None,
            &cancel,
            "fixture-nonce",
            || Ok(()),
            |url| {
                let params: BTreeMap<_, _> = url.query_pairs().collect();
                let path = format!(
                "/auth/callback?state={}&code=accepted&client_id=fixture-client&scope={PLAN_SCOPE}",
                params["state"]
            );
                send_callback(&params["redirect_uri"], &path, None);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(account.subject, "fixture-subject");
    assert!(!saved.plan_enabled());
    account.subject = "another-registration".into();
    assert!(provider
        .renew(
            &account,
            &saved,
            &cancel,
            Instant::now() + Duration::from_secs(3)
        )
        .is_err());
    assert_eq!(worker.join().unwrap().len(), 4);
}
