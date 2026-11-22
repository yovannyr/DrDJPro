use super::*;
use serde_json::json;

pub(super) const MAX_TOOLS: usize = 32;

/// Convert only documented status codes; never surface arbitrary response text containing a key.
pub(super) fn vendor_error(error: &Value) -> Option<AiError> {
    for field in ["code", "type", "status"] {
        let mapped = match error[field].as_str() {
            Some("subscription_sharing_usage_limit_exceeded") => AiError::PlanUsageLimit,
            Some("subscription_sharing_usage_unavailable") => AiError::PlanUsageUnavailable,
            Some("subscription_sharing_user_not_eligible") => AiError::PlanNotEligible,
            Some("insufficient_quota" | "billing_hard_limit_reached" | "billing_error") => {
                AiError::Quota
            }
            Some("rate_limit_exceeded" | "rate_limit_error" | "RESOURCE_EXHAUSTED") => {
                AiError::RateLimit
            }
            Some(
                "invalid_api_key"
                | "authentication_error"
                | "permission_error"
                | "UNAUTHENTICATED"
                | "PERMISSION_DENIED",
            ) => AiError::Authentication,
            Some("model_not_found" | "not_found_error" | "NOT_FOUND") => AiError::ModelNotFound,
            Some("overloaded_error" | "UNAVAILABLE") => AiError::Unavailable,
            _ => continue,
        };
        return Some(mapped);
    }
    None
}

pub(super) fn payload(
    protocol: AiProtocol,
    mut url: Url,
    req: &AiRequest,
) -> Result<(Url, Option<Value>, bool), AiError> {
    use AiProtocol::*;
    use AiRequest::*;
    if let Test { model, embedding } = req {
        let probe = if *embedding {
            Embed {
                model: model.clone(),
                input: ".".into(),
            }
        } else {
            Chat {
                model: model.clone(),
                instruction: String::new(),
                input: "Hi".into(),
                tools: Vec::new(),
                max_tokens: if protocol == ChatGptPlan { 16 } else { 1 },
                stream: false,
            }
        };
        return payload(protocol, url, &probe);
    }
    let is_public_openai = url.host_str() == Some("api.openai.com");
    // Mistral's native chat schema has no stream_options field.
    let stream_usage_option = url.host_str() != Some("api.mistral.ai");
    let mut path = url
        .path_segments_mut()
        .map_err(|_| AiError::InvalidEndpoint)?;
    path.pop_if_empty();
    let mut sse = false;
    let body = match req {
        Models => {
            path.push("models");
            None
        }
        Embed { model, input } => {
            check_model(model)?;
            if input.len() > MAX_REQUEST {
                return Err(AiError::InvalidRequest);
            }
            Some(match protocol {
                OpenAi => {
                    path.push("embeddings");
                    json!({"model":model,"input":input})
                }
                Gemini => {
                    let model = gemini_model(model)?;
                    path.push("models").push(&format!("{model}:embedContent"));
                    json!({"model":format!("models/{model}"),"content":{"parts":[{"text":input}]}})
                }
                _ => return Err(AiError::Unsupported),
            })
        }
        Chat {
            model,
            instruction,
            input,
            tools,
            max_tokens,
            stream,
        } => {
            check_model(model)?;
            if *max_tokens == 0
                || *max_tokens > 32768
                || tools.len() > MAX_TOOLS
                || instruction.len().saturating_add(input.len()) > MAX_REQUEST
                || tools
                    .iter()
                    .any(|t| !valid_name(&t.name) || !t.parameters.is_object())
            {
                return Err(AiError::InvalidRequest);
            }
            sse = *stream || protocol == ChatGptPlan;
            let funcs: Vec<_> = tools.iter().map(|t| json!({"name":t.name,"description":t.description,"parameters":t.parameters})).collect();
            let mut obj;
            match protocol {
                OpenAi => {
                    path.push("chat").push("completions");
                    let mut messages = Vec::new();
                    if !instruction.is_empty() {
                        messages.push(json!({"role":"system","content":instruction}));
                    }
                    messages.push(json!({"role":"user","content":input}));
                    obj = json!({"model":model,"messages":messages,"stream":sse});
                    obj[if is_public_openai {
                        "max_completion_tokens"
                    } else {
                        "max_tokens"
                    }] = json!(max_tokens);
                    if sse && stream_usage_option {
                        obj["stream_options"] = json!({"include_usage":true});
                    }
                    if !funcs.is_empty() {
                        obj["tools"] = json!(funcs
                            .into_iter()
                            .map(|f| json!({"type":"function","function":f}))
                            .collect::<Vec<_>>());
                    }
                }
                Anthropic => {
                    path.push("messages");
                    obj = json!({"model":model,"messages":[{"role":"user","content":input}],"max_tokens":max_tokens,"stream":sse});
                    if !instruction.is_empty() {
                        obj["system"] = json!(instruction);
                    }
                    if !tools.is_empty() {
                        obj["tools"]=json!(tools.iter().map(|t|json!({"name":t.name,"description":t.description,"input_schema":t.parameters})).collect::<Vec<_>>());
                    }
                }
                Gemini => {
                    let op = if sse {
                        "streamGenerateContent"
                    } else {
                        "generateContent"
                    };
                    path.push("models")
                        .push(&format!("{}:{op}", gemini_model(model)?));
                    obj = json!({"contents":[{"role":"user","parts":[{"text":input}]}],"generationConfig":{"maxOutputTokens":max_tokens}});
                    if !instruction.is_empty() {
                        obj["systemInstruction"] = json!({"parts":[{"text":instruction}]});
                    }
                    if !funcs.is_empty() {
                        obj["tools"] = json!([{"functionDeclarations":funcs}]);
                    }
                }
                ChatGptPlan => {
                    path.push("responses");
                    obj = json!({"model":model,"store":false,"stream":true,"input":[{"role":"user","content":input}],"max_output_tokens":max_tokens});
                    if !instruction.is_empty() {
                        obj["instructions"] = json!(instruction);
                    }
                    if !funcs.is_empty() {
                        obj["tools"] = json!(funcs
                            .into_iter()
                            .map(|mut f| {
                                f["type"] = json!("function");
                                f
                            })
                            .collect::<Vec<_>>());
                    }
                }
            }
            Some(obj)
        }
        Test { .. } => return Err(AiError::InvalidRequest),
    };
    drop(path);
    if sse && protocol == Gemini {
        url.query_pairs_mut().append_pair("alt", "sse");
    }
    if matches!(req, Models) {
        match protocol {
            Gemini => {
                url.query_pairs_mut().append_pair("pageSize", "256");
            }
            Anthropic => {
                url.query_pairs_mut().append_pair("limit", "256");
            }
            _ => (),
        }
    }
    Ok((url, body, sse))
}

fn check_model(model: &str) -> Result<(), AiError> {
    if model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        Err(AiError::InvalidRequest)
    } else {
        Ok(())
    }
}

fn gemini_model(model: &str) -> Result<&str, AiError> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    if model.is_empty()
        || matches!(model, "." | "..")
        || !model
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(AiError::InvalidRequest);
    }
    Ok(model)
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c))
}

pub(super) fn decode(protocol: AiProtocol, req: &AiRequest, v: &Value) -> Result<AiReply, AiError> {
    let probe = matches!(
        req,
        AiRequest::Test {
            embedding: false,
            ..
        }
    );
    if let Some(e) = v.get("error") {
        return Err(vendor_error(e).unwrap_or(AiError::Malformed));
    }
    let mut reply = AiReply {
        tokens: usage(protocol, v),
        ..Default::default()
    };
    match req {
        AiRequest::Models => {
            let (collection, field) = match protocol {
                AiProtocol::OpenAi | AiProtocol::Anthropic => ("data", "id"),
                AiProtocol::Gemini => ("models", "name"),
                AiProtocol::ChatGptPlan => ("models", "slug"),
            };
            for model in array(&v[collection])? {
                if protocol == AiProtocol::ChatGptPlan && model["visibility"] != "list" {
                    continue;
                }
                let id = string(&model[field])?;
                let id = if protocol == AiProtocol::Gemini {
                    id.strip_prefix("models/").unwrap_or(id)
                } else {
                    id
                };
                if check_model(id).is_ok() {
                    reply.models.push(id.to_owned());
                }
                if reply.models.len() == 256 {
                    break;
                }
            }
        }
        AiRequest::Embed { .. }
        | AiRequest::Test {
            embedding: true, ..
        } => {
            let vector = if protocol == AiProtocol::Gemini {
                &v["embedding"]["values"]
            } else {
                &v["data"][0]["embedding"]
            };
            let vector = array(vector)?;
            if vector.is_empty() || vector.len() > 65536 {
                return Err(AiError::Malformed);
            }
            for n in vector {
                let n = n.as_f64().ok_or(AiError::Malformed)? as f32;
                if !n.is_finite() {
                    return Err(AiError::Malformed);
                }
                reply.embedding.push(n);
            }
        }
        _ => match protocol {
            AiProtocol::OpenAi => {
                let reason = &v["choices"][0]["finish_reason"];
                check_stop(protocol, reason, probe)?;
                let msg = v["choices"][0].get("message").ok_or(AiError::Malformed)?;
                if let Some(content) = msg["content"].as_str() {
                    reply.text.push_str(content);
                }
                if let Some(calls) = msg["tool_calls"].as_array() {
                    for call in calls {
                        let args = serde_json::from_str(string(&call["function"]["arguments"])?)
                            .map_err(|_| AiError::Malformed)?;
                        push_call(
                            &mut reply,
                            string(&call["id"])?,
                            string(&call["function"]["name"])?,
                            args,
                        )?;
                    }
                }
                if msg["content"].as_str().is_none()
                    && reply.tool_calls.is_empty()
                    && !(probe && reason == "length")
                {
                    return Err(AiError::Malformed);
                }
            }
            AiProtocol::Anthropic => {
                check_stop(protocol, &v["stop_reason"], probe)?;
                let content = array(&v["content"])?;
                for part in content {
                    match part["type"].as_str() {
                        Some("text") => reply.text.push_str(string(&part["text"])?),
                        Some("tool_use") => push_call(
                            &mut reply,
                            string(&part["id"])?,
                            string(&part["name"])?,
                            part["input"].clone(),
                        )?,
                        _ => (),
                    }
                }
                if content.is_empty() && !(probe && v["stop_reason"] == "max_tokens") {
                    return Err(AiError::Malformed);
                }
            }
            AiProtocol::Gemini => {
                check_stop(protocol, &v["candidates"][0]["finishReason"], probe)?;
                gemini_parts(v, &mut reply, &mut |_| {})?;
            }
            AiProtocol::ChatGptPlan => return Err(AiError::Malformed),
        },
    }
    Ok(reply)
}

/// A tiny connection probe verifies inference even if its single output token is exhausted.
/// Truncated, filtered or unsupported replies are never delivered as completed feature answers.
pub(super) fn check_stop(protocol: AiProtocol, reason: &Value, probe: bool) -> Result<(), AiError> {
    use AiProtocol::*;
    match (protocol, string(reason)?) {
        (OpenAi, "stop" | "tool_calls")
        | (Anthropic, "end_turn" | "stop_sequence" | "tool_use")
        | (Gemini, "STOP") => Ok(()),
        (OpenAi, "length") | (Anthropic, "max_tokens") | (Gemini, "MAX_TOKENS") if probe => Ok(()),
        _ => Err(AiError::Malformed),
    }
}

pub(super) fn array(v: &Value) -> Result<&Vec<Value>, AiError> {
    v.as_array().ok_or(AiError::Malformed)
}
pub(super) fn string(v: &Value) -> Result<&str, AiError> {
    v.as_str().ok_or(AiError::Malformed)
}

pub(super) fn push_call(
    reply: &mut AiReply,
    id: &str,
    name: &str,
    args: Value,
) -> Result<(), AiError> {
    if reply.tool_calls.len() >= MAX_TOOLS
        || id.len() > 256
        || !valid_name(name)
        || !args.is_object()
    {
        return Err(AiError::Malformed);
    }
    reply.tool_calls.push(AiToolCall {
        name: name.to_owned(),
        id: id.to_owned(),
        arguments: args,
    });
    Ok(())
}

pub(super) fn gemini_parts(
    v: &Value,
    reply: &mut AiReply,
    on_delta: &mut dyn FnMut(&str),
) -> Result<(), AiError> {
    let candidate = v["candidates"].get(0).ok_or(AiError::Malformed)?;
    if let Some(parts) = candidate["content"]["parts"].as_array() {
        for part in parts {
            if part["thought"].as_bool() == Some(true) {
                continue;
            }
            if let Some(text) = part["text"].as_str() {
                reply.text.push_str(text);
                on_delta(text);
            }
            if let Some(call) = part.get("functionCall") {
                push_call(
                    reply,
                    call["id"].as_str().unwrap_or(""),
                    string(&call["name"])?,
                    call["args"].clone(),
                )?;
            }
        }
    } else if candidate["finishReason"].is_null() {
        return Err(AiError::Malformed);
    }
    Ok(())
}

pub(super) fn usage(protocol: AiProtocol, v: &Value) -> u64 {
    match protocol {
        AiProtocol::OpenAi | AiProtocol::ChatGptPlan => {
            v["usage"]["total_tokens"].as_u64().unwrap_or(0)
        }
        AiProtocol::Gemini => v["usageMetadata"]["totalTokenCount"].as_u64().unwrap_or(0),
        AiProtocol::Anthropic => [
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
        .into_iter()
        .filter_map(|field| v["usage"][field].as_u64())
        .fold(0u64, u64::saturating_add),
    }
}
