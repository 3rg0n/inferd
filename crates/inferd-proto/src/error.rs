//! Error types for the wire-format crate.

use serde::{Deserialize, Serialize};

/// Machine-readable error classification carried on `error` response frames.
///
/// See `docs/protocol-v1.md` §"Response stream" and ADR 0008 for the rationale
/// behind exposing this distinct from the human-readable `message` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Admission queue full at submit time. Caller may retry immediately or with backoff.
    QueueFull,
    /// Selected backend errored before or during generation. Caller may retry.
    BackendUnavailable,
    /// Request failed validation. Caller should not retry without changing the request.
    InvalidRequest,
    /// Frame exceeded the 64 MiB cap. Connection is closed.
    FrameTooLarge,
    /// Daemon-side bug or unexpected condition.
    Internal,
}

/// Errors produced by the proto crate while parsing or validating frames.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// A single NDJSON frame exceeded `MAX_FRAME_BYTES`.
    #[error("frame exceeds {} byte cap", crate::MAX_FRAME_BYTES)]
    FrameTooLarge,

    /// I/O error reading the underlying transport.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Frame bytes were not valid JSON.
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),

    /// Frame parsed but failed semantic validation.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Length-prefixed framing was malformed: an unknown frame-type byte,
    /// a length varint that didn't terminate within its budget, or a
    /// stream that ended mid-frame (after the length prefix, before the
    /// full payload). The byte stream is no longer trustworthy; the
    /// caller closes the connection. (ADR 0021.)
    #[error("malformed frame: {0}")]
    MalformedFrame(String),
}

impl ProtoError {
    /// Map a parse/validate error to the wire-level `ErrorCode` a daemon should
    /// emit on the response stream.
    pub fn to_error_code(&self) -> ErrorCode {
        match self {
            ProtoError::FrameTooLarge => ErrorCode::FrameTooLarge,
            ProtoError::Decode(_)
            | ProtoError::InvalidRequest(_)
            | ProtoError::MalformedFrame(_) => ErrorCode::InvalidRequest,
            ProtoError::Io(_) => ErrorCode::Internal,
        }
    }
}
