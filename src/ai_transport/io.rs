//! Interruptible HTTP on the calling provider worker. Only the socket and timer
//! futures run on Tokio. The synchronous JSON/SSE parsers and callbacks run outside it.
use super::{AiError, Budget};
use bytes::{Buf, Bytes};
use std::{cell::RefCell, future::Future, io, ops::Deref, time::Duration};
use tokio::runtime::{Builder, Runtime};

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

fn retire() {
    // An idle current-thread runtime would leave Hyper's socket tasks parked
    // until the next request. Shutting it down drops those tasks immediately.
    RUNTIME.with(|slot| {
        if let Some(runtime) = slot.borrow_mut().take() {
            runtime.shutdown_background();
        }
    });
}

fn wait<T>(
    budget: Budget<'_>,
    future: impl Future<Output = Result<T, reqwest::Error>>,
) -> Result<T, AiError> {
    let result = RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        let runtime = match slot.as_ref() {
            Some(runtime) => runtime,
            None => slot.insert(
                Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .map_err(|_| AiError::Network)?,
            ),
        };
        runtime.block_on(async {
            let mut pending = std::pin::pin!(future);
            loop {
                let period = budget.remaining()?.min(Duration::from_millis(25));
                if let Ok(done) = tokio::time::timeout(period, pending.as_mut()).await {
                    budget.remaining()?;
                    return done.map_err(|e| {
                        if e.is_timeout() {
                            AiError::Timeout
                        } else if e.is_connect() {
                            AiError::Unavailable
                        } else {
                            AiError::Network
                        }
                    });
                }
            }
        })
    });
    if matches!(result, Err(AiError::Cancelled | AiError::Timeout)) {
        retire();
    }
    result
}

pub(crate) struct Response<'a> {
    inner: reqwest::Response,
    chunk: Bytes,
    budget: Budget<'a>,
}

impl Deref for Response<'_> {
    type Target = reqwest::Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl io::Read for Response<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.budget.remaining().map_err(io::Error::other)?;
        while self.chunk.is_empty() {
            match wait(self.budget, self.inner.chunk()).map_err(io::Error::other)? {
                Some(chunk) => self.chunk = chunk,
                None => return Ok(0),
            }
        }
        let len = self.chunk.len().min(buf.len());
        self.chunk.copy_to_slice(&mut buf[..len]);
        Ok(len)
    }
}

impl Drop for Response<'_> {
    fn drop(&mut self) {
        // Cancellation can also arrive in a streamed callback between reads.
        if self.budget.remaining().is_err() {
            retire();
        }
    }
}

pub(crate) fn send(
    req: reqwest::RequestBuilder,
    budget: Budget<'_>,
) -> Result<Response<'_>, AiError> {
    let remaining = budget.remaining()?;
    let inner = wait(budget, async move { req.timeout(remaining).send().await })?;
    Ok(Response {
        inner,
        chunk: Bytes::new(),
        budget,
    })
}
