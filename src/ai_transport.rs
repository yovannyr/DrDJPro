//! Bounded provider HTTP. Call from the provider worker, never from deck/UI callbacks.
use reqwest::header::HeaderValue;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use url::{Host, Url};

pub(crate) mod io;
#[cfg(test)]
mod io_tests;
mod protocol;
mod stream;
#[cfg(test)]
pub(crate) mod tests;

const MAX_BODY: usize = 1024 * 1024;
const MAX_REQUEST: usize = 256 * 1024;
const TEST_TIME: Duration = Duration::from_secs(8);
const REQUEST_TIME: Duration = Duration::from_secs(30);
const DISCOVERY_TIME: Duration = Duration::from_millis(730);
const LOCAL_ENDPOINTS: [(&str, &str); 3] = [
    ("Ollama", "http://127.0.0.1:11434/v1"),
    ("LM Studio", "http://127.0.0.1:1234/v1"),
    ("llama.cpp", "http://127.0.0.1:8080/v1"),
];
/// Public resource authorized for ChatGPT subscription access.
pub const PLAN_BASE: &str = "https://api.openai.com/v1";

/// Wire protocol of a configured endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiProtocol {
    OpenAi,
    Anthropic,
    Gemini,
    ChatGptPlan,
}

/// Stable UI errors. Raw headers, bodies and URLs never escape as diagnostic messages.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum AiError {
    #[error("Invalid provider endpoint or credential transport")]
    InvalidEndpoint,
    #[error("Invalid or oversized request")]
    InvalidRequest,
    #[error("Authentication failed")]
    Authentication,
    #[error("Network error")]
    Network,
    #[error("Request timed out")]
    Timeout,
    #[error("Model not found")]
    ModelNotFound,
    #[error("Rate limit reached")]
    RateLimit,
    #[error("Provider quota exhausted")]
    Quota,
    #[error("ChatGPT plan usage limit reached (subscription_sharing_usage_limit_exceeded)")]
    PlanUsageLimit,
    #[error("ChatGPT plan usage unavailable (subscription_sharing_usage_unavailable)")]
    PlanUsageUnavailable,
    #[error("ChatGPT plan usage is unavailable for this account or workspace (subscription_sharing_user_not_eligible)")]
    PlanNotEligible,
    #[error("AI settings changed. Try again.")]
    SettingsChanged,
    #[error("Malformed, oversized or incomplete provider response")]
    Malformed,
    #[error("Provider unavailable")]
    Unavailable,
    #[error("Request cancelled")]
    Cancelled,
    #[error("Operation unsupported by this provider")]
    Unsupported,
}

/// Read-only description of a tool. The transport never executes it.
#[derive(Clone, Serialize)]
pub struct AiTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A suggested tool invocation; a caller must separately validate and authorize it.
#[derive(Clone, PartialEq)]
pub struct AiToolCall {
    pub name: String,
    pub id: String,
    pub arguments: Value,
}

/// An operation already approved by the router.
pub enum AiRequest {
    Models,
    Test {
        model: String,
        embedding: bool,
    },
    Chat {
        model: String,
        instruction: String,
        input: String,
        tools: Vec<AiTool>,
        max_tokens: u32,
        stream: bool,
    },
    Embed {
        model: String,
        input: String,
    },
}

#[derive(Default)]
pub struct AiReply {
    pub text: String,
    pub models: Vec<String>,
    pub embedding: Vec<f32>,
    pub tool_calls: Vec<AiToolCall>,
    /// Sum of the provider-reported input and output usage, if reported.
    pub tokens: u64,
}

pub struct AiDiscovery {
    pub name: &'static str,
    pub base_url: &'static str,
    pub models: Vec<String>,
}

/// Parse the provider's base, independent of any vendor preset. Never honor URL credentials.
pub fn validate_base_url(input: &str) -> Result<Url, AiError> {
    if input.len() > 2048
        || input.contains('\\')
        || input.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AiError::InvalidEndpoint);
    }
    let (_, rest) = input.split_once("://").ok_or(AiError::InvalidEndpoint)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let url = Url::parse(input).map_err(|_| AiError::InvalidEndpoint)?;
    if authority.is_empty()
        || authority.contains('@')
        || !matches!(url.scheme(), "https" | "http")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AiError::InvalidEndpoint);
    }
    Ok(url)
}

/// Locality is a property of the destination, not a user-editable provider flag.
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        _ => false,
    }
}

/// Each request, including one retry, consumes a single wall-clock budget.
pub fn execute(
    protocol: AiProtocol,
    base: &str,
    secret: Option<&str>,
    request: &AiRequest,
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(&str),
) -> Result<AiReply, AiError> {
    let budget = if matches!(request, AiRequest::Test { .. } | AiRequest::Models) {
        TEST_TIME
    } else {
        REQUEST_TIME
    };
    execute_until(
        protocol,
        base,
        secret,
        request,
        cancel,
        Instant::now() + budget,
        on_delta,
    )
}

/// Share a fixed deadline across credential refresh, retries, and inference.
pub(crate) fn execute_until(
    protocol: AiProtocol,
    base: &str,
    secret: Option<&str>,
    request: &AiRequest,
    cancel: &AtomicBool,
    deadline: Instant,
    on_delta: &mut dyn FnMut(&str),
) -> Result<AiReply, AiError> {
    let budget = Budget {
        until: deadline,
        cancelled: cancel,
    };
    budget.remaining()?;
    AiProvider::new(protocol, base, secret)?.request(request, &budget, &mut || Ok(()), on_delta)
}

#[derive(Clone, Copy)]
pub(crate) struct Budget<'a> {
    pub(crate) until: Instant,
    pub(crate) cancelled: &'a AtomicBool,
}
impl<'a> Budget<'a> {
    #[cfg(test)]
    fn new(duration: Duration, cancelled: &'a AtomicBool) -> Self {
        Self {
            until: Instant::now() + duration,
            cancelled,
        }
    }
    fn remaining(&self) -> Result<Duration, AiError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(AiError::Cancelled);
        }
        self.until
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(AiError::Timeout)
    }
}

fn client(base: &Url, budget: &Budget<'_>) -> Result<Client, AiError> {
    let mut builder = Client::builder()
        .tls_backend_native()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(budget.remaining()?.min(Duration::from_secs(2)));
    if is_loopback(base) {
        builder = builder.no_proxy();
        // Do not delegate the reserved name to DNS or a configurable proxy.
        if base.host_str() == Some("localhost") {
            let port = base
                .port_or_known_default()
                .ok_or(AiError::InvalidEndpoint)?;
            builder = builder.resolve_to_addrs(
                "localhost",
                &[
                    SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
                    SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
                ],
            );
        }
    }
    builder.build().map_err(|_| AiError::Network)
}

/// A validated destination and its protocol adapter. Secrets are only borrowed for a worker operation.
pub struct AiProvider<'a> {
    protocol: AiProtocol,
    base: Url,
    secret: Option<&'a str>,
}
impl<'a> AiProvider<'a> {
    pub(crate) fn new(
        protocol: AiProtocol,
        base: &str,
        secret: Option<&'a str>,
    ) -> Result<Self, AiError> {
        let base = validate_base_url(base)?;
        if (secret.is_some() && base.scheme() == "http" && !is_loopback(&base))
            || (protocol == AiProtocol::ChatGptPlan
                && base.as_str().strip_suffix('/').unwrap_or(base.as_str()) != PLAN_BASE)
        {
            return Err(AiError::InvalidEndpoint);
        }
        if protocol == AiProtocol::ChatGptPlan && secret.is_none() {
            return Err(AiError::Authentication);
        }
        if secret.is_some_and(|s| {
            s.is_empty() || s.len() > 32768 || !s.bytes().all(|b| b.is_ascii_graphic())
        }) {
            return Err(AiError::Authentication);
        }
        Ok(Self {
            protocol,
            base,
            secret,
        })
    }
    pub(crate) fn dispatch(
        &self,
        request: &AiRequest,
        cancel: &AtomicBool,
        deadline: Instant,
        before_send: &mut dyn FnMut() -> Result<(), AiError>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<AiReply, AiError> {
        self.request(
            request,
            &Budget {
                until: deadline,
                cancelled: cancel,
            },
            before_send,
            on_delta,
        )
    }
    fn request(
        &self,
        request: &AiRequest,
        budget: &Budget<'_>,
        before_send: &mut dyn FnMut() -> Result<(), AiError>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<AiReply, AiError> {
        let Self {
            protocol,
            base,
            secret,
        } = self;
        let protocol = *protocol;
        let (url, body, streaming) = protocol::payload(protocol, base.clone(), request)?;
        let client = client(base, budget)?;
        let mut builder = if let Some(body) = body {
            let bytes = serde_json::to_vec(&body).map_err(|_| AiError::InvalidRequest)?;
            if bytes.len() > MAX_REQUEST {
                return Err(AiError::InvalidRequest);
            }
            client
                .post(url)
                .header("content-type", "application/json")
                .body(bytes)
        } else {
            client.get(url)
        };
        if protocol == AiProtocol::Anthropic {
            builder = builder.header("anthropic-version", "2023-06-01");
        }
        if let Some(secret) = secret {
            let (name, header) = match protocol {
                AiProtocol::Gemini => ("x-goog-api-key", HeaderValue::from_str(secret)),
                AiProtocol::Anthropic => ("x-api-key", HeaderValue::from_str(secret)),
                _ => (
                    "authorization",
                    HeaderValue::from_str(&zeroize::Zeroizing::new(format!("Bearer {secret}"))),
                ),
            };
            let mut key = header.map_err(|_| AiError::Authentication)?;
            key.set_sensitive(true);
            builder = builder.header(name, key);
        }
        let mut retried = false;
        loop {
            budget.remaining()?;
            before_send()?;
            let request_builder = builder.try_clone().ok_or(AiError::InvalidRequest)?;
            let mut response = io::send(request_builder, *budget)?;
            budget.remaining()?;
            if response.status().is_success() {
                if response
                    .content_length()
                    .is_some_and(|len| len > MAX_BODY as u64)
                {
                    return Err(AiError::Malformed);
                }
                return if streaming {
                    stream::parse(protocol, response, budget, on_delta)
                } else {
                    let bytes = read_bytes(&mut response, MAX_BODY, budget)?;
                    protocol::decode(
                        protocol,
                        request,
                        &serde_json::from_slice(&bytes).map_err(|_| AiError::Malformed)?,
                    )
                };
            }
            let status = response.status().as_u16();
            let delay = retry_delay(&response);
            let error = http_error(status, &read_bytes(&mut response, 16384, budget)?);
            if retried
                || !matches!(error, AiError::RateLimit | AiError::Unavailable)
                || delay > Duration::from_secs(1)
                || delay >= budget.remaining()?
            {
                return Err(error);
            }
            drop(response);
            let resume = Instant::now() + delay;
            while let Some(left) = resume.checked_duration_since(Instant::now()) {
                budget.remaining()?;
                std::thread::sleep(left.min(Duration::from_millis(25)));
            }
            retried = true;
        }
    }
}

fn retry_delay(response: &reqwest::Response) -> Duration {
    match response.headers().get("retry-after") {
        Some(value) => Duration::from_secs(
            value
                .to_str()
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(u64::MAX),
        ),
        None => Duration::from_millis(100),
    }
}

fn http_error(status: u16, body: &[u8]) -> AiError {
    let json: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if let Some(special) = protocol::vendor_error(json.get("error").unwrap_or(&json)) {
        return special;
    }
    match status {
        401 | 403 => AiError::Authentication,
        402 => AiError::Quota,
        404 => AiError::ModelNotFound,
        408 | 504 => AiError::Timeout,
        429 => AiError::RateLimit,
        500 | 502 | 503 | 529 => AiError::Unavailable,
        _ => AiError::Malformed,
    }
}

fn read_bytes(
    reader: &mut impl Read,
    limit: usize,
    budget: &Budget<'_>,
) -> Result<Vec<u8>, AiError> {
    let mut out = Vec::new();
    let mut buf = [0; 8192];
    loop {
        budget.remaining()?;
        let size = reader.read(&mut buf).map_err(|e| io_error(budget, &e))?;
        if size == 0 {
            budget.remaining()?;
            return Ok(out);
        }
        if size > limit - out.len() {
            return Err(AiError::Malformed);
        }
        out.extend_from_slice(&buf[..size]);
    }
}

fn io_error(budget: &Budget<'_>, err: &std::io::Error) -> AiError {
    budget
        .remaining()
        .err()
        .or_else(|| err.get_ref()?.downcast_ref::<AiError>().copied())
        .unwrap_or(if err.kind() == std::io::ErrorKind::TimedOut {
            AiError::Timeout
        } else {
            AiError::Network
        })
}

/// Probe exactly three conventional loopback endpoints, with one overall budget below 3s.
pub fn discover_local(cancel: &AtomicBool) -> Vec<AiDiscovery> {
    discover_with(|base_url, deadline| {
        execute_until(
            AiProtocol::OpenAi,
            base_url,
            None,
            &AiRequest::Models,
            cancel,
            deadline,
            &mut |_| {},
        )
    })
}

fn discover_with(
    mut probe: impl FnMut(&'static str, Instant) -> Result<AiReply, AiError>,
) -> Vec<AiDiscovery> {
    let mut found = Vec::new();
    let deadline = Instant::now() + 3 * DISCOVERY_TIME;
    for (name, base_url) in LOCAL_ENDPOINTS {
        if let Ok(reply) = probe(base_url, deadline.min(Instant::now() + DISCOVERY_TIME)) {
            found.push(AiDiscovery {
                name,
                base_url,
                models: reply.models,
            });
        }
    }
    found
}
