//! `Backend` trait and shared types.

use async_trait::async_trait;
use inferd_proto::v2::{ResolvedV2, StopReasonV2, ToolCallId, ToolUseInput, UsageV2};
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

/// One event in a v2 generation stream — typed-content-block surface
/// per ADR 0015.
///
/// v2 separates user-visible text (`Text`) from reasoning trace
/// (`Thinking`) and emits complete tool-call requests (`ToolUse`) as
/// their own variant rather than raw tokens. Backends that don't
/// distinguish thinking content (any non-Gemma-4 backend) emit only
/// `Text` events.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenEventV2 {
    /// Incremental user-visible text.
    Text(String),
    /// Incremental reasoning trace (Gemma 4 `<|think|>` content).
    Thinking(String),
    /// Complete tool-call request emitted by the model.
    ToolUse {
        /// Identifier paired with the consumer's eventual ToolResult.
        tool_call_id: ToolCallId,
        /// Tool name from the request's `tools[]` table.
        name: String,
        /// JSON arguments emitted by the model.
        input: ToolUseInput,
    },
    /// Final event for a successful generation.
    Done {
        /// Reason generation stopped.
        stop_reason: StopReasonV2,
        /// Token-count usage.
        usage: UsageV2,
    },
}

/// Stream of `TokenEventV2` values produced by a backend during a v2
/// generation. Dropping the stream cancels the in-flight generation.
pub type TokenStreamV2 = Pin<Box<dyn Stream<Item = TokenEventV2> + Send>>;

/// Per-backend capability advertisement. The daemon consults this on
/// boot to decide whether v2 multimodal / tool-use requests can be
/// dispatched, and reports the advertised set on the admin status
/// surface so middleware authors can introspect what the running
/// daemon can do without trial-and-error.
///
/// Per the v0.2 plan: until cloud adapters land, the only adapters
/// shipped are `mock` and `llamacpp`. Both opt-in selectively —
/// `mock` for tests, `llamacpp` once Phase 3+ wires mtmd / tool
/// parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackendCapabilities {
    /// `true` if the backend implements `generate_v2` (typed
    /// content blocks, tool definitions). When `false` the daemon's
    /// v2 dispatch falls back to `Error{Internal,
    /// "v2 not supported by this backend"}`.
    pub v2: bool,
    /// `true` if the backend can ingest image attachments. Reported
    /// to consumers; requests with image content blocks against a
    /// non-image backend get `Error{AttachmentUnsupported,...}`.
    pub vision: bool,
    /// `true` if the backend can ingest audio attachments.
    pub audio: bool,
    /// `true` if the backend can ingest video attachments. (Reserved.)
    pub video: bool,
    /// `true` if the backend natively supports tool-use round-tripping
    /// (parses `<|tool_call>` from token stream, accepts `tool_result`
    /// blocks in the next request, etc.).
    pub tools: bool,
    /// `true` if the backend separates `<|think|>` reasoning trace
    /// from user-visible output.
    pub thinking: bool,
}

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

    /// Capabilities the backend advertises to the daemon and (via
    /// the admin status surface) to consumers. Default: text-only v1
    /// backend, no v2, no multimodal, no tools — matches the v0.1
    /// `mock` and `llamacpp` shape so existing implementors compile
    /// unchanged.
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    /// Begin a generation and return a stream of `TokenEvent` values.
    ///
    /// Errors returned here surface as `Response::Error` *before* any tokens
    /// reach the client. Errors that occur after the first token has streamed
    /// terminate the stream without a `Done`.
    async fn generate(&self, req: Resolved) -> Result<TokenStream, GenerateError>;

    /// Begin a v2 generation and return a stream of `TokenEventV2`
    /// values. Default impl returns `GenerateError::Internal("v2 not
    /// supported by this backend")` — adapters opt in by overriding.
    /// The daemon checks `capabilities().v2` before calling this on
    /// the v2 path; the default `false` capability prevents dispatch
    /// from reaching here for non-v2 backends.
    async fn generate_v2(&self, _req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        Err(GenerateError::Internal(
            "v2 not supported by this backend".into(),
        ))
    }

    /// Best-effort graceful shutdown. The daemon calls this on stop; the
    /// adapter should release model memory, terminate worker threads, and
    /// any other long-lived resources within the deadline.
    async fn stop(&self, _timeout: std::time::Duration) -> Result<(), GenerateError> {
        Ok(())
    }
}
