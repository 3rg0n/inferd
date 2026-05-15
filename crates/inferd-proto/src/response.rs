//! Response frame schema.

use crate::error::ErrorCode;
use serde::{Deserialize, Serialize};

/// Why a generation ended. Carried on `done` frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    /// Model emitted the end-of-turn token cleanly.
    End,
    /// `max_tokens` reached.
    Length,
    /// Caller disconnected or otherwise cancelled.
    Cancelled,
    /// Generation aborted; partial output may be in `Response::content`.
    Error,
}

/// Token-count usage report carried on `done` frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    pub prompt_tokens: u32,
    /// Tokens generated in the response.
    pub completion_tokens: u32,
}

/// One frame on the response NDJSON stream.
///
/// Variant fields map directly to the wire shape documented in
/// `docs/protocol-v1.md` §"Response stream".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Response {
    /// Lifecycle transition not tied to token output. `id` is `"admin"` for
    /// status broadcast on the admin socket; otherwise the request id.
    Status {
        /// Correlation id; `"admin"` for non-request status.
        id: String,
        /// Lifecycle state name (`loading_model`, `ready`, `restarting`, `draining`).
        status: String,
    },
    /// One incremental generated token.
    Token {
        /// Request id.
        id: String,
        /// Token text.
        content: String,
    },
    /// Terminal frame for a successful generation.
    Done {
        /// Request id.
        id: String,
        /// Full generated text.
        content: String,
        /// Token-count usage.
        usage: Usage,
        /// Why generation stopped.
        stop_reason: StopReason,
        /// `Backend::name()` of the adapter that served this request.
        ///
        /// Diagnostic only — apps must not branch on this (ADR 0007).
        backend: String,
    },
    /// Terminal frame for a failed generation.
    Error {
        /// Request id.
        id: String,
        /// Machine-readable classification.
        code: ErrorCode,
        /// Human-readable description.
        message: String,
    },
}

impl Response {
    /// Correlation id of the frame regardless of variant.
    pub fn id(&self) -> &str {
        match self {
            Response::Status { id, .. }
            | Response::Token { id, .. }
            | Response::Done { id, .. }
            | Response::Error { id, .. } => id,
        }
    }

    /// `true` if this frame ends a request stream (`done` or `error`).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Response::Done { .. } | Response::Error { .. })
    }
}
