use super::*;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;

pub(crate) struct WireRequest {
    pub(crate) line: String,
    headers: String,
    pub(crate) body: Value,
}

/// Serve complete HTTP responses over an ephemeral loopback port. Observe what actually left the adapter.
pub(crate) fn server(replies: Vec<String>) -> (String, thread::JoinHandle<Vec<WireRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TEST_TIME)).unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut headers = String::new();
            loop {
                let mut header = String::new();
                assert!(reader.read_line(&mut header).unwrap() > 0);
                if header == "\r\n" {
                    break;
                }
                headers.push_str(&header.to_ascii_lowercase());
            }
            let len = headers
                .lines()
                .find_map(|h| h.strip_prefix("content-length: "))
                .map_or(0, |s| s.parse::<usize>().unwrap());
            let mut bytes = vec![0; len];
            reader.read_exact(&mut bytes).unwrap();
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            requests.push(WireRequest {
                line,
                headers,
                body,
            });
            let _ = stream.write_all(reply.as_bytes());
        }
        requests
    });
    (url, handle)
}

pub(crate) fn http(status: u16, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} test\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    )
}

fn run(protocol: AiProtocol, base: &str, req: &AiRequest) -> Result<AiReply, AiError> {
    execute(
        protocol,
        base,
        Some("mock-key"),
        req,
        &AtomicBool::new(false),
        &mut |_| {},
    )
}

fn chat(stream: bool) -> AiRequest {
    AiRequest::Chat {
        model: "demo".into(),
        instruction: "Read metadata only.".into(),
        input: "Find tracks".into(),
        max_tokens: 32,
        stream,
        tools: vec![AiTool {
            name: "search".into(),
            description: "Find tracks by title".into(),
            parameters: json!({"type":"object","properties":{"title":{"type":"string"}}}),
        }],
    }
}

#[test]
fn endpoints_are_structural_and_secret_transport_is_enforced_before_io() {
    for bad in [
        "",
        "localhost:8000",
        "ftp://host",
        "http://",
        "https://@host",
        "https://user@host/v1",
        "https://host/?",
        "https://host/#",
        "https://host\\@fake",
        "http:// /x",
    ] {
        assert!(validate_base_url(bad).is_err(), "{bad}");
    }
    for local in [
        "http://127.0.0.2/v1",
        "https://LOCALHOST",
        "http://[::1]:1234/v1",
        "http://[::ffff:127.0.0.1]:8080/v1",
    ] {
        assert!(is_loopback(&validate_base_url(local).unwrap()));
    }
    for key in ["", " has-space", "header\tinjection", "line\r\nbreak"] {
        assert_eq!(
            AiProvider::new(AiProtocol::OpenAi, PLAN_BASE, Some(key)).err(),
            Some(AiError::Authentication)
        );
    }
    for cloud in [
        "http://localhost.example/v1",
        "http://192.168.2.2/v1",
        "http://[::ffff:192.168.2.2]/v1",
    ] {
        assert!(!is_loopback(&validate_base_url(cloud).unwrap()));
        assert_eq!(
            run(AiProtocol::OpenAi, cloud, &AiRequest::Models).err(),
            Some(AiError::InvalidEndpoint)
        );
    }
    for base in [
        "http://127.0.0.1/v1",
        "https://other.example/v1",
        "https://api.openai.com/v1//",
    ] {
        assert_eq!(
            run(AiProtocol::ChatGptPlan, base, &AiRequest::Models).err(),
            Some(AiError::InvalidEndpoint)
        );
    }
}

#[test]
fn actual_minimal_inference_uses_native_route_headers_and_model() {
    for (protocol, route, header, response) in [
        (
            AiProtocol::OpenAi,
            "/v1/chat/completions",
            "authorization: bearer mock-key",
            json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}),
        ),
        (
            AiProtocol::Anthropic,
            "/v1/messages",
            "x-api-key: mock-key",
            json!({"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}),
        ),
        (
            AiProtocol::Gemini,
            "/v1/models/demo:generateContent",
            "x-goog-api-key: mock-key",
            json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"ok"}]}}]}),
        ),
    ] {
        let (url, server) = server(vec![http(200, "", &response.to_string())]);
        let reply = run(
            protocol,
            &url,
            &AiRequest::Test {
                model: "demo".into(),
                embedding: false,
            },
        )
        .unwrap();
        assert_eq!(reply.text, "ok");
        let req = server.join().unwrap().pop().unwrap();
        assert_eq!(req.line.trim_end(), format!("POST {route} HTTP/1.1"));
        assert!(req.headers.contains(header));
        assert!(!req.body.to_string().contains("mock-key"));
        if protocol == AiProtocol::Anthropic {
            assert!(req.headers.contains("anthropic-version: 2023-06-01"));
        }
        match protocol {
            AiProtocol::OpenAi | AiProtocol::Anthropic => {
                assert_eq!(req.body["max_tokens"], 1);
                assert_eq!(req.body["messages"][0]["content"], "Hi");
                assert_eq!(req.body["model"], "demo");
            }
            _ => assert_eq!(req.body["generationConfig"]["maxOutputTokens"], 1),
        }
    }
}

#[test]
fn model_list_limits_and_plan_visibility() {
    for (proto, body, expected) in [
        (AiProtocol::OpenAi, json!({"data":[]}), vec![]),
        (
            AiProtocol::Anthropic,
            json!({"data":[{"id":"claude"}]}),
            vec!["claude"],
        ),
        (
            AiProtocol::Gemini,
            json!({"models":[{"name":"models/example"}]}),
            vec!["example"],
        ),
    ] {
        let (url, server) = server(vec![http(200, "", &body.to_string())]);
        assert_eq!(
            run(proto, &url, &AiRequest::Models).unwrap().models,
            expected
        );
        let req = server.join().unwrap().pop().unwrap();
        let suffix = match proto {
            AiProtocol::Anthropic => "?limit=256",
            AiProtocol::Gemini => "?pageSize=256",
            _ => "",
        };
        assert_eq!(req.line, format!("GET /v1/models{suffix} HTTP/1.1\r\n"));
        assert!(req.body.is_null());
    }
    let many = (0..600)
        .map(|i| json!({"id":format!("m{i}")}))
        .collect::<Vec<_>>();
    assert_eq!(
        protocol::decode(
            AiProtocol::OpenAi,
            &AiRequest::Models,
            &json!({"data":many})
        )
        .unwrap()
        .models
        .len(),
        256
    );
    let models = json!({"models":[{"slug":"a","visibility":"list"},{"slug":"b","visibility":"hide"},{"slug":"c"}]});
    assert_eq!(
        protocol::decode(AiProtocol::ChatGptPlan, &AiRequest::Models, &models)
            .unwrap()
            .models,
        vec!["a"]
    );
}

#[test]
fn embedding_routes_and_malformed_vectors() {
    let input = AiRequest::Embed {
        model: "demo".into(),
        input: "label".into(),
    };
    for (proto, route, response) in [
        (
            AiProtocol::OpenAi,
            "/v1/embeddings",
            json!({"data":[{"embedding":[0.25,0.75]}]}),
        ),
        (
            AiProtocol::Gemini,
            "/v1/models/demo:embedContent",
            json!({"embedding":{"values":[0.25,0.75]}}),
        ),
    ] {
        let (url, server) = server(vec![http(200, "", &response.to_string())]);
        assert_eq!(run(proto, &url, &input).unwrap().embedding, [0.25, 0.75]);
        assert_eq!(
            server.join().unwrap()[0].line,
            format!("POST {route} HTTP/1.1\r\n")
        );
    }
    assert_eq!(
        protocol::decode(
            AiProtocol::OpenAi,
            &input,
            &json!({"data":[{"embedding":[1e100]}]})
        )
        .err(),
        Some(AiError::Malformed)
    );
    assert_eq!(
        protocol::payload(
            AiProtocol::Anthropic,
            validate_base_url(PLAN_BASE).unwrap(),
            &input
        )
        .err(),
        Some(AiError::Unsupported)
    );
}

#[test]
fn redirect_is_not_followed_with_credentials() {
    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    let location = format!(
        "Location: http://{}/capture\r\n",
        trap.local_addr().unwrap()
    );
    let (url, server) = server(vec![http(307, &location, "")]);
    assert_eq!(
        run(AiProtocol::OpenAi, &url, &AiRequest::Models).err(),
        Some(AiError::Malformed)
    );
    assert_eq!(server.join().unwrap().len(), 1);
    trap.set_nonblocking(true).unwrap();
    assert_eq!(
        trap.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn bounded_retry_only_for_retryable_statuses() {
    let (url, mock) = server(vec![
        http(429, "Retry-After: 0\r\n", "{}"),
        http(200, "", r#"{"data":[]}"#),
    ]);
    assert!(run(AiProtocol::OpenAi, &url, &AiRequest::Models).is_ok());
    assert_eq!(mock.join().unwrap().len(), 2);
    for (status, headers, body, err) in [
        (401, "", "secret echo", AiError::Authentication),
        (404, "", "{}", AiError::ModelNotFound),
        (
            429,
            "",
            r#"{"error":{"code":"insufficient_quota"}}"#,
            AiError::Quota,
        ),
        (429, "Retry-After: 300\r\n", "{}", AiError::RateLimit),
        (200, "", "<html>upstream error</html>", AiError::Malformed),
    ] {
        let (url, mock) = server(vec![http(status, headers, body)]);
        assert_eq!(
            run(AiProtocol::OpenAi, &url, &AiRequest::Models).err(),
            Some(err)
        );
        assert_eq!(mock.join().unwrap().len(), 1);
    }
    let (url, mock) = server(vec![
        http(503, "Retry-After: 0\r\n", "{}"),
        http(503, "", "{}"),
    ]);
    assert_eq!(
        run(AiProtocol::OpenAi, &url, &AiRequest::Models).err(),
        Some(AiError::Unavailable)
    );
    assert_eq!(mock.join().unwrap().len(), 2);
}

#[test]
fn retry_rechecks_current_policy_before_another_network_call() {
    let (url, mock) = server(vec![http(429, "Retry-After: 0\r\n", "{}")]);
    let mut attempts = 0;
    let provider = AiProvider::new(AiProtocol::OpenAi, &url, None).unwrap();
    let result = provider.dispatch(
        &AiRequest::Models,
        &AtomicBool::new(false),
        Instant::now() + TEST_TIME,
        &mut || {
            attempts += 1;
            if attempts > 1 {
                Err(AiError::SettingsChanged)
            } else {
                Ok(())
            }
        },
        &mut |_| {},
    );
    assert_eq!(result.err(), Some(AiError::SettingsChanged));
    assert_eq!(attempts, 2);
    assert_eq!(mock.join().unwrap().len(), 1);
}

#[test]
fn malformed_oversized_and_cancelled_answers_do_not_succeed() {
    let (url, server) = server(vec![format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    )]);
    assert_eq!(
        run(AiProtocol::OpenAi, &url, &AiRequest::Models).err(),
        Some(AiError::Malformed)
    );
    server.join().unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        execute(
            AiProtocol::OpenAi,
            "bad",
            None,
            &AiRequest::Models,
            &cancel,
            &mut |_| {}
        )
        .err(),
        Some(AiError::Cancelled)
    );
    cancel.store(false, Ordering::Release);
    let budget = Budget::new(TEST_TIME, &cancel);
    let bytes = vec![b'x'; MAX_BODY + 1];
    assert_eq!(
        stream::parse(AiProtocol::Gemini, &bytes[..], &budget, &mut |_| {}).err(),
        Some(AiError::Malformed)
    );
}

fn frames(events: &[Value]) -> String {
    events.iter().map(|v| format!("data: {v}\n\n")).collect()
}

#[test]
fn discovery_has_only_fixed_loopback_destinations_and_a_single_short_budget() {
    let mut probes = Vec::new();
    let start = Instant::now();
    let results = discover_with(|endpoint, deadline| {
        assert!(is_loopback(&validate_base_url(endpoint).unwrap()));
        assert!(deadline <= start + Duration::from_secs(3));
        probes.push(endpoint);
        if probes.len() == 2 {
            return Err(AiError::Timeout);
        }
        Ok(AiReply {
            models: vec!["demo".into()],
            ..Default::default()
        })
    });
    assert_eq!(
        probes,
        [
            "http://127.0.0.1:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.1:8080/v1"
        ]
    );
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].name, "Ollama");
    assert_eq!(results[1].base_url, probes[2]);
    assert_eq!(results[1].models, ["demo"]);
    assert!(discover_local(&AtomicBool::new(true)).is_empty());
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn cancellation_after_first_stream_delta_discards_remaining_frames() {
    let events = frames(&[
        json!({"choices":[{"delta":{"content":"one"}}]}),
        json!({"choices":[{"delta":{"content":"two"},"finish_reason":"stop"}]}),
    ]) + "data: [DONE]\n\n";
    let cancel = AtomicBool::new(false);
    let mut received = String::new();
    let err = stream::parse(
        AiProtocol::OpenAi,
        events.as_bytes(),
        &Budget::new(TEST_TIME, &cancel),
        &mut |delta| {
            received.push_str(delta);
            cancel.store(true, Ordering::Release);
        },
    )
    .err();
    assert_eq!(err, Some(AiError::Cancelled));
    assert_eq!(received, "one");
}

#[test]
fn streamed_openai_tool_fragments_and_total_usage_traverse_http() {
    let mut events = frames(&[
        json!({"choices":[{"delta":{"content":"A"}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"search","arguments":"{\"title\":"}}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Blue\"}"}}]},"finish_reason":"tool_calls"}]}),
        json!({"choices":[],"usage":{"total_tokens":37}}),
    ]);
    events.push_str("data: [DONE]\n\n");
    let (url, server) = server(vec![http(
        200,
        "Content-Type: text/event-stream\r\n",
        &events,
    )]);
    let mut fragments = Vec::new();
    let reply = execute(
        AiProtocol::OpenAi,
        &url,
        None,
        &chat(true),
        &AtomicBool::new(false),
        &mut |s| fragments.push(s.to_owned()),
    )
    .unwrap();
    assert_eq!(reply.tokens, 37);
    assert_eq!(fragments, ["A"]);
    assert_eq!(reply.tool_calls[0].arguments, json!({"title":"Blue"}));
    let req = server.join().unwrap().pop().unwrap();
    assert_eq!(req.body["tools"][0]["function"]["name"], "search");
    assert_eq!(req.body["stream_options"]["include_usage"], true);
}

#[test]
fn incomplete_streams_do_not_return_partial_success() {
    let partial = frames(&[json!({"choices":[{"delta":{"content":"partial"}}]})]);
    let (url, server) = server(vec![http(200, "", &partial)]);
    let mut streamed = String::new();
    let result = execute(
        AiProtocol::OpenAi,
        &url,
        None,
        &chat(true),
        &AtomicBool::new(false),
        &mut |s| streamed.push_str(s),
    );
    assert_eq!(streamed, "partial");
    assert_eq!(result.err(), Some(AiError::Malformed));
    server.join().unwrap();
}

#[test]
fn truncated_probe_verifies_connection_but_incomplete_or_filtered_chat_never_succeeds() {
    let probe = AiRequest::Test {
        model: "demo".into(),
        embedding: false,
    };
    for (protocol, reason, body) in [
        (
            AiProtocol::OpenAi,
            "length",
            json!({"choices":[{"finish_reason":"length","message":{"content":"part"}}]}),
        ),
        (
            AiProtocol::Anthropic,
            "max_tokens",
            json!({"stop_reason":"max_tokens","content":[{"type":"text","text":"part"}]}),
        ),
        (
            AiProtocol::Gemini,
            "MAX_TOKENS",
            json!({"candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"part"}]}}]}),
        ),
    ] {
        assert!(protocol::decode(protocol, &probe, &body).is_ok());
        assert_eq!(
            protocol::decode(protocol, &chat(false), &body).err(),
            Some(AiError::Malformed)
        );
        for reason in [reason, "SAFETY", "content_filter", "refusal", "unknown"] {
            let (event, final_event) = match protocol {
                AiProtocol::OpenAi => (
                    json!({"choices":[{"finish_reason":reason,"delta":{"content":"part"}}]}),
                    "data: [DONE]\n\n",
                ),
                AiProtocol::Anthropic => (
                    json!({"type":"message_delta","delta":{"stop_reason":reason}}),
                    "data: {\"type\":\"message_stop\"}\n\n",
                ),
                _ => (json!({"candidates":[{"finishReason":reason}]}), ""),
            };
            let data = frames(&[event]) + final_event;
            assert_eq!(
                stream::parse(
                    protocol,
                    data.as_bytes(),
                    &Budget::new(TEST_TIME, &AtomicBool::new(false)),
                    &mut |_| {}
                )
                .err(),
                Some(AiError::Malformed)
            );
        }
    }
    let refused = json!({"choices":[{"finish_reason":"content_filter","message":{"content":""}}]});
    assert_eq!(
        protocol::decode(AiProtocol::OpenAi, &probe, &refused).err(),
        Some(AiError::Malformed)
    );
}

#[test]
fn anthropic_gemini_and_plan_streams_decode_their_native_tool_schema() {
    let anthropic = frames(&[
        json!({"type":"message_start","message":{"usage":{"input_tokens":5,"cache_read_input_tokens":2}}}),
        json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"A"}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","name":"search","id":"tool","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ]);
    let gemini = frames(&[
        json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"private"},{"text":"A"}]}}]}),
        json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"functionCall":{"name":"search","args":{}}}]}}],"usageMetadata":{"totalTokenCount":10}}),
    ]);
    let plan = frames(&[
        json!({"type":"response.output_text.delta","delta":"A"}),
        json!({"type":"response.completed","response":{"error":null,"usage":{"total_tokens":10},"output":[{"type":"function_call","call_id":"c1","name":"search","arguments":"{}"}]}}),
    ]);
    for (proto, events) in [
        (AiProtocol::Anthropic, anthropic),
        (AiProtocol::Gemini, gemini),
        (AiProtocol::ChatGptPlan, plan),
    ] {
        let cancel = AtomicBool::new(false);
        let mut delta = String::new();
        let reply = stream::parse(
            proto,
            events.as_bytes(),
            &Budget::new(TEST_TIME, &cancel),
            &mut |s| delta.push_str(s),
        )
        .unwrap();
        assert_eq!(delta, "A");
        assert_eq!(reply.tokens, 10);
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "search");
        assert_eq!(reply.tool_calls[0].arguments, json!({}));
    }
}

#[test]
fn plan_has_bound_resource_store_policy_and_specific_errors() {
    let (url, body, stream) = protocol::payload(
        AiProtocol::ChatGptPlan,
        validate_base_url(PLAN_BASE).unwrap(),
        &chat(false),
    )
    .unwrap();
    let body = body.unwrap();
    assert_eq!(url.as_str(), "https://api.openai.com/v1/responses");
    assert!(stream);
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["type"], "function");
    let err = json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_unavailable","message":"sensitive data"}}});
    assert_eq!(
        stream::parse(
            AiProtocol::ChatGptPlan,
            frames(&[err]).as_bytes(),
            &Budget::new(TEST_TIME, &AtomicBool::new(false)),
            &mut |_| {}
        )
        .err(),
        Some(AiError::PlanUsageUnavailable)
    );
    assert_eq!(
        http_error(
            429,
            br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#
        ),
        AiError::PlanUsageLimit
    );
}

#[test]
fn mistral_omits_unsupported_optional_stream_options() {
    let (url, body, streaming) = protocol::payload(
        AiProtocol::OpenAi,
        validate_base_url("https://api.mistral.ai/v1").unwrap(),
        &chat(true),
    )
    .unwrap();
    assert!(streaming);
    assert_eq!(url.as_str(), "https://api.mistral.ai/v1/chat/completions");
    let body = body.unwrap();
    assert!(body.get("stream_options").is_none());
    assert_eq!(body["max_tokens"], 32);
}

#[test]
fn stalled_request_respects_total_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            if let Ok((_socket, _)) = listener.accept() {
                thread::sleep(Duration::from_millis(350));
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let started = Instant::now();
    let cancel = AtomicBool::new(false);
    let result = execute_until(
        AiProtocol::Gemini,
        &base,
        None,
        &AiRequest::Models,
        &cancel,
        Instant::now() + Duration::from_millis(200),
        &mut |_| {},
    );
    assert_eq!(result.err(), Some(AiError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(3));
    handle.join().unwrap();
}
