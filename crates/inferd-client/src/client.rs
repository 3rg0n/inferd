//! Shared client error type for every inferd wire surface.
//!
//! As of v0.4 (ADR 0021) the text-only v1 generation client was
//! removed when v1 was folded into v2; the v2 generation client lives
//! in [`crate::v2_client`] and the embed client in
//! [`crate::embed_client`]. Both reuse [`ClientError`] defined here so
//! the connect-and-retry helper ([`crate::dial_and_wait_ready`]) has a
//! single transport-error taxonomy to match against.

use std::io;

/// Errors produced by the inference / embed clients.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Connection / I/O error against the daemon.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// JSON encode/decode of a wire frame failed.
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
    /// Connection closed before a terminal `done` / `error` frame.
    /// Callers treat this as an error equivalent to
    /// `code: backend_unavailable` and apply their own retry policy
    /// (ADR 0007 — caller owns retry).
    #[error("daemon closed connection before terminal frame")]
    UnexpectedEof,
    /// Length-prefixed framing was malformed (unknown frame-type byte,
    /// a length varint that didn't terminate, or an oversize frame).
    /// The byte stream is no longer trustworthy (ADR 0021).
    #[error("malformed frame: {0}")]
    MalformedFrame(String),
}
