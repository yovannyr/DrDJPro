//! Bounded textual upload on a control worker. Never called by an audio callback
//! or panic/signal hook. Binary dumps remain local: text consent is not dump consent.
use super::runtime_log::{emit, Event};
use super::{privacy::sanitize_stack, CrashReport, CrashStore};
use flate2::{write::GzEncoder, Compression};
use reqwest::blocking::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    time::Duration,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ACK: u64 = 4096;

/// Results for a single bounded pass; failed reports retain their queued consent.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct UploadSummary {
    pub submitted: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Pollable worker result. Dropping cancels requests not yet started and never
/// joins/blocks the caller. An in-flight request has an eight-second deadline.
pub struct UploadTask {
    result: Receiver<io::Result<UploadSummary>>,
    cancel: Arc<AtomicBool>,
}

impl UploadTask {
    /// Request cancellation before subsequent requests; never waits for HTTP.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    /// Poll from a control/UI thread without waiting for network or file I/O.
    pub fn try_result(&self) -> Result<io::Result<UploadSummary>, TryRecvError> {
        self.result.try_recv()
    }
}

impl Drop for UploadTask {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Start one upload pass, including durable retries from previous runs. Call again
/// at the next startup (normal app-start/UI wiring is a separate slice). An OS file
/// lock excludes simultaneous passes for this store, without blocking the caller.
/// Pending reports are authorized only by explicit auto-submit settings; declined
/// reports stay local. Queued reports already have explicit retry consent.
pub fn spawn_upload_queue(store: CrashStore) -> io::Result<UploadTask> {
    spawn_with(store, client_builder())
}

fn spawn_with(store: CrashStore, builder: ClientBuilder) -> io::Result<UploadTask> {
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&cancel);
    let (sender, result) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("crash-upload".into())
        .spawn(move || {
            let outcome = run_queue(&store, builder, &stop);
            if outcome.is_err() {
                emit(Event::UploadPassFailed, None, None);
            }
            let _ = sender.send(outcome);
        })?;
    Ok(UploadTask { result, cancel })
}

fn client_builder() -> ClientBuilder {
    Client::builder()
        .tls_backend_native()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(2))
        .timeout(REQUEST_TIMEOUT)
        .user_agent("DRDJPro-Rust/0.1.0 crash-report")
}

pub(super) fn endpoint(raw: &str) -> io::Result<url::Url> {
    let url = url::Url::parse(raw).map_err(|_| invalid("Invalid crash HTTPS endpoint"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(invalid(
            "Crash endpoint requires HTTPS without credentials/query/fragment",
        ));
    }
    Ok(url)
}

fn run_queue(
    store: &CrashStore,
    builder: ClientBuilder,
    cancel: &AtomicBool,
) -> io::Result<UploadSummary> {
    let _upload_lock = store.try_upload_lock()?;
    if cancel.load(Ordering::Acquire) {
        return Ok(UploadSummary::default());
    }
    let destination = store.upload_destination()?;
    let client = builder
        .build()
        .map_err(|_| invalid("Cannot initialize crash HTTPS client"))?;
    // Uses the store's existing count/byte limits, not additional queue files.
    store.enforce_limits()?;
    let reports = store.list()?;
    let mut summary = UploadSummary::default();
    for report in reports {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        // Claim and acknowledge against this pass's destination under the store
        // lock. No lock is held across HTTPS, and a pass never changes its URL.
        let Some(report) = store.prepare_upload(&report.id, &destination)? else {
            summary.skipped += 1;
            continue;
        };
        match upload(&client, &destination.url, &report, cancel, || {
            store.may_send(&report, &destination)
        }) {
            Ok(true) if store.acknowledge(&report, &destination)? => {
                summary.submitted += 1;
                emit(Event::UploadAcknowledged, Some(&report.id), None);
            }
            Ok(_) => summary.skipped += 1,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => break,
            Err(error) => {
                summary.failed += 1;
                // Errors contain only our fixed classifications, never a remote
                // response, URL, TLS details, paths or arbitrary report payload.
                emit(Event::UploadRetry, Some(&report.id), Some(error.kind()));
            }
        }
    }
    Ok(summary)
}

#[derive(Serialize)]
struct TextReport<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<&'a super::BuildIdentity>,
    schema_version: u8,
    id: &'a str,
    created_ms: i64,
    version: String,
    os: String,
    arch: String,
    stack: String,
}

// These are generated identifiers, not user annotations. Revalidate bounded
// characters because persisted JSON can have been edited outside the app.
fn identifier(value: &str) -> String {
    value
        .chars()
        .take(64)
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
        .collect()
}

fn upload(
    client: &Client,
    endpoint: &url::Url,
    report: &CrashReport,
    cancel: &AtomicBool,
    may_send: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    let payload = TextReport {
        build: report.build.as_ref(),
        schema_version: 1,
        id: &report.id,
        created_ms: report.created_ms,
        version: identifier(&report.version),
        os: identifier(&report.os),
        arch: identifier(&report.arch),
        stack: sanitize_stack(&report.stack),
    };
    // The redacted stack is bounded (256 lines, 512 characters/line). Stream JSON
    // into gzip in memory; never read a .dmp or create a .gz staging file on disk.
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut gzip, &payload).map_err(io::Error::other)?;
    let body = gzip.finish()?;
    if !may_send()? {
        return Ok(false);
    }
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Upload cancelled",
        ));
    }
    let response = client
        .post(endpoint.clone())
        .header("Content-Type", "application/json")
        .header("Content-Encoding", "gzip")
        .header("Accept", "application/json")
        .header("Idempotency-Key", &report.id)
        .body(body)
        .send()
        .map_err(|_| invalid("HTTPS request failed or timed out"))?;
    if !response.status().is_success() {
        return Err(invalid("Server did not acknowledge crash report"));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_ACK + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("Cannot read crash acknowledgement"))?;
    if bytes.len() as u64 > MAX_ACK {
        return Err(invalid("Crash acknowledgement exceeds limit"));
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ack {
        id: String,
        accepted: bool,
    }
    let ack: Ack =
        serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid crash acknowledgement"))?;
    if ack.id != report.id || !ack.accepted {
        return Err(invalid("Mismatched or rejected crash acknowledgement"));
    }
    Ok(true)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests;
