//! AWS Bedrock-runtime InvokeModelWithResponseStream backend (Phase
//! 6B-5).
//!
//! Wraps the AWS Bedrock-runtime [InvokeModelWithResponseStream] API
//! behind the [`Backend`](crate::Backend) trait. v0.2.0 ships only the
//! **Anthropic-on-Bedrock** body shape — Claude models invoked via
//! Bedrock's pinned `anthropic_version: "bedrock-2023-05-31"` payload.
//! Titan / Llama / Mistral body shapes (and the Bedrock Converse API,
//! which is one-API-many-models) are deferred to v0.2.x+ per the
//! locked v0.2.0 backend matrix.
//!
//! [InvokeModelWithResponseStream]: https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InvokeModelWithResponseStream.html
//!
//! # Auth
//!
//! Two modes, in this order:
//! 1. **Bearer token** when `AWS_BEARER_TOKEN_BEDROCK` is set
//!    (or another env var name nominated in config). Sent as
//!    `Authorization: Bearer <value>`. Bedrock rolled this short-form
//!    auth out in 2025-06; preferred when available because it skips
//!    the SigV4 path entirely.
//! 2. **SigV4** otherwise, using the standard AWS credential chain
//!    (`AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` + optional
//!    `AWS_SESSION_TOKEN`). The signing keys are derived per-request
//!    against the configured region; service is always `bedrock`.
//!
//! Cross-account assume-role is out of scope for v0.2.0 — operators
//! who need it set the env vars from their own session before starting
//! the daemon.
//!
//! # Streaming
//!
//! Bedrock's response is the AWS event-stream framing format
//! (`Content-Type: application/vnd.amazon.eventstream`) — *not* SSE.
//! Each event is a length-prefixed binary frame containing a JSON
//! payload. The [`eventstream`] module parses this to a sequence of
//! `chunk { bytes: <base64> }` events whose decoded body is the
//! Anthropic SSE-shaped JSON we'd see from `api.anthropic.com`
//! directly.
//!
//! # What we deliberately don't do
//!
//! - No retries (ADR 0007: caller owns retry).
//! - No mid-stream failover (ADR 0007: structurally broken).
//! - No HTTP server inbound (ADR 0006: outbound only).
//! - No multimodal in v0.2.0 — `image` / `audio` / `video` content
//!   blocks are rejected at request build time. The Anthropic body
//!   *does* support image inputs; multimodal lands in a follow-up.
//! - No Converse API. Operators wanting the Converse one-call-many-
//!   models ergonomics should use a higher-level proxy.

mod adapter;
mod body;
mod eventstream;
mod sigv4;

pub use adapter::{BedrockAuth, BedrockInvoke, BedrockInvokeConfig, BedrockInvokeError};
