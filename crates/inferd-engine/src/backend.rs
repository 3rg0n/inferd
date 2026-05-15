//! `Backend` trait and shared types.

use async_trait::async_trait;
use inferd_proto::{Resolved, StopReason, Usage};
use std::pin::Pin;
use tokio_stream::Stream;

/// One event in a generation stream.
///
/// A successful generation produces zero or more `Token` events terminated by
/// exactly one `Done`. A failed generation produces zero or more `Token`
/// events followed by no further events; the adapter returns the failure as
/// a `GenerateError` from `generate()` (pre-stream) or terminates the stream
/// without a `Done` (mid-stream) — see ADR 0007 for the failure-semantics
/// contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenEvent {
    /// One incremental generated token.
    Token(String),
    /// Final event for a successful generation.
    Done {
        /// Reason generation stopped.
        stop_reason: StopReason,
        /// Token-count usage.
        usage: Usage,
    },
}

/// Stream of `TokenEvent` values produced by a backend during generation.
///
/// Dropping the stream cancels the in-flight generation. Adapters must wire
/// drop to their underlying cancellation primitive (e.g. a `CancellationToken`
/// or by aborting the spawned task).
pub type TokenStream = Pin<Box<dyn Stream<Item = TokenEvent> + Send>>;

/// Errors returned by `Backend::generate()` *before* any tokens have streamed.
///
/// Mid-stream failures terminate the stream silently (no `Done` event); the
/// caller observes the absence of a terminal event and translates that to
/// `Response::Error` with `code: backend_unavailable` per `docs/protocol-v1.md`.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// Backend was not ready when `generate()` was called.
    #[error("backend not ready")]
    NotReady,
    /// Backend rejected the request as malformed (sampling out of range, etc.).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Backend tried to start generation and failed (model not loaded,
    /// remote API errored, etc.).
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// Anything else.
    #[error("internal: {0}")]
    Internal(String),
}

/// An inference backend.
///
/// Implementations are owned by the daemon and shared across requests through
/// `Arc<dyn Backend>`. Methods take `&self`; concurrent invocations of
/// `generate()` are serialised by the daemon's admission queue, not by the
/// trait.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Stable identifier for the backend, e.g. `"mock"`, `"llamacpp"`,
    /// `"anthropic"`. Echoed in `Response::Done::backend` for diagnostic
    /// purposes (ADR 0007).
    fn name(&self) -> &str;

    /// Whether the backend has finished its boot sequence and can serve
    /// requests. The daemon does not create its inference listener until
    /// every registered backend reports `true` (see `THREAT_MODEL.md` F-13).
    fn ready(&self) -> bool;

    /// Begin a generation and return a stream of `TokenEvent` values.
    ///
    /// Errors returned here surface as `Response::Error` *before* any tokens
    /// reach the client. Errors that occur after the first token has streamed
    /// terminate the stream without a `Done`.
    async fn generate(&self, req: Resolved) -> Result<TokenStream, GenerateError>;

    /// Best-effort graceful shutdown. The daemon calls this on stop; the
    /// adapter should release model memory, terminate worker threads, and
    /// any other long-lived resources within the deadline.
    async fn stop(&self, _timeout: std::time::Duration) -> Result<(), GenerateError> {
        Ok(())
    }
}
