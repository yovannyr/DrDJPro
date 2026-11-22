use super::{protocol::*, *};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

/// Parse complete SSE data frames. A disconnected partial stream never becomes a successful answer.
pub(super) fn parse(
    protocol: AiProtocol,
    input: impl Read,
    budget: &Budget<'_>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<AiReply, AiError> {
    let mut reader = BufReader::new(input.take((MAX_BODY + 1) as u64));
    let mut bytes = 0;
    let mut line = String::new();
    let mut data = String::new();
    let mut state = Completion::default();
    loop {
        budget.remaining()?;
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                AiError::Malformed
            } else {
                io_error(budget, &e)
            }
        })?;
        if n == 0 {
            break;
        }
        bytes += n;
        if bytes > MAX_BODY {
            return Err(AiError::Malformed);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !data.is_empty() {
                budget.remaining()?;
                state.frame(protocol, &data, on_delta)?;
                data.clear();
                if state.done && protocol != AiProtocol::Gemini {
                    break;
                }
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    budget.remaining()?;
    if !data.is_empty() || !state.done {
        return Err(AiError::Malformed);
    }
    for call in state.pending.into_values() {
        let args = if call.arguments.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&call.arguments).map_err(|_| AiError::Malformed)?
        };
        push_call(&mut state.reply, &call.id, &call.name, args)?;
    }
    Ok(state.reply)
}

#[derive(Default)]
struct Completion {
    reply: AiReply,
    pending: BTreeMap<u64, PendingCall>,
    done: bool,
    finished_generation: bool,
    input_tokens: u64,
}
impl Completion {
    fn call(&mut self, idx: u64) -> Result<&mut PendingCall, AiError> {
        if !self.pending.contains_key(&idx) && self.pending.len() >= MAX_TOOLS {
            return Err(AiError::Malformed);
        }
        Ok(self.pending.entry(idx).or_default())
    }
    fn delta(&mut self, text: &str, callback: &mut dyn FnMut(&str)) {
        self.reply.text.push_str(text);
        callback(text);
    }
    fn frame(
        &mut self,
        protocol: AiProtocol,
        data: &str,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<(), AiError> {
        if protocol == AiProtocol::OpenAi && data == "[DONE]" {
            self.done = self.finished_generation;
            return if self.done {
                Ok(())
            } else {
                Err(AiError::Malformed)
            };
        }
        let v: Value = serde_json::from_str(data).map_err(|_| AiError::Malformed)?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(vendor_error(e).unwrap_or(AiError::Malformed));
        }
        match protocol {
            AiProtocol::OpenAi => {
                self.reply.tokens = self.reply.tokens.max(usage(protocol, &v));
                let choices = array(&v["choices"])?;
                if let Some(choice) = choices.first() {
                    if !choice["finish_reason"].is_null() {
                        check_stop(protocol, &choice["finish_reason"], false)?;
                        self.finished_generation = true;
                    }
                    if let Some(text) = choice["delta"]["content"].as_str() {
                        self.delta(text, on_delta);
                    }
                    if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                        for c in calls {
                            let call = self.call(c["index"].as_u64().ok_or(AiError::Malformed)?)?;
                            if let Some(id) = c["id"].as_str() {
                                call.id.push_str(id);
                            }
                            if let Some(name) = c["function"]["name"].as_str() {
                                call.name.push_str(name);
                            }
                            if let Some(args) = c["function"]["arguments"].as_str() {
                                call.arguments.push_str(args);
                            }
                        }
                    }
                }
            }
            AiProtocol::Anthropic => match string(&v["type"])? {
                "message_start" => {
                    let u = &v["message"]["usage"];
                    self.input_tokens = [
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                    ]
                    .into_iter()
                    .filter_map(|k| u[k].as_u64())
                    .fold(0, u64::saturating_add);
                    self.reply.tokens = self
                        .input_tokens
                        .saturating_add(u["output_tokens"].as_u64().unwrap_or(0));
                }
                "content_block_start" if v["content_block"]["type"] == "tool_use" => {
                    let c = &v["content_block"];
                    let call = self.call(v["index"].as_u64().ok_or(AiError::Malformed)?)?;
                    call.id = string(&c["id"])?.into();
                    call.name = string(&c["name"])?.into();
                    if c["input"].as_object().is_some_and(|obj| !obj.is_empty()) {
                        call.arguments = c["input"].to_string();
                    }
                }
                "content_block_delta" => match string(&v["delta"]["type"])? {
                    "text_delta" => self.delta(string(&v["delta"]["text"])?, on_delta),
                    "input_json_delta" => {
                        let idx = v["index"].as_u64().ok_or(AiError::Malformed)?;
                        self.pending
                            .get_mut(&idx)
                            .ok_or(AiError::Malformed)?
                            .arguments
                            .push_str(string(&v["delta"]["partial_json"])?);
                    }
                    _ => (),
                },
                "message_delta" => {
                    if !v["delta"]["stop_reason"].is_null() {
                        check_stop(protocol, &v["delta"]["stop_reason"], false)?;
                        self.finished_generation = true;
                    }
                    self.reply.tokens = self
                        .input_tokens
                        .saturating_add(v["usage"]["output_tokens"].as_u64().unwrap_or(0))
                }
                "message_stop" if self.finished_generation => self.done = true,
                "message_stop" => return Err(AiError::Malformed),
                "error" => return Err(AiError::Unavailable),
                _ => (),
            },
            AiProtocol::Gemini => {
                if !v["candidates"][0]["finishReason"].is_null() {
                    check_stop(protocol, &v["candidates"][0]["finishReason"], false)?;
                    self.done = true;
                }
                gemini_parts(&v, &mut self.reply, on_delta)?;
                self.reply.tokens = self.reply.tokens.max(usage(protocol, &v));
            }
            AiProtocol::ChatGptPlan => match string(&v["type"])? {
                "response.output_text.delta" => self.delta(string(&v["delta"])?, on_delta),
                "response.completed" => {
                    let response = &v["response"];
                    if response.get("status").is_some_and(|s| s != "completed") {
                        return Err(AiError::Malformed);
                    }
                    if let Some(e) = response.get("error").filter(|e| !e.is_null()) {
                        return Err(vendor_error(e).unwrap_or(AiError::Malformed));
                    }
                    for item in array(&response["output"])? {
                        if item["type"] == "function_call" {
                            let args = serde_json::from_str(string(&item["arguments"])?)
                                .map_err(|_| AiError::Malformed)?;
                            push_call(
                                &mut self.reply,
                                string(&item["call_id"])?,
                                string(&item["name"])?,
                                args,
                            )?;
                        }
                    }
                    self.reply.tokens = usage(protocol, response);
                    self.done = true;
                }
                "response.failed" | "response.incomplete" | "error" => {
                    return Err(vendor_error(&v["response"]["error"])
                        .or_else(|| vendor_error(&v))
                        .unwrap_or(AiError::Malformed))
                }
                _ => (),
            },
        }
        Ok(())
    }
}
