//! OpenAI-compat outbound HTTP backend (Phase 5A).
//!
//! Wraps the OpenAI Chat Completions surface (and any provider that
//! implements the same wire — vLLM, LM Studio, LocalAI, llama.cpp's
//! HTTP server, OpenRouter) behind the [`Backend`](crate::Backend)
//! trait.
//!
//! # Scope (v0.2)
//!
//! - Text-only v2 generation: `messages[]` of `Text` blocks roundtrips
//!   through `messages: [{role, content}]`.
//! - Tool-use round-tripping: `Tool` declarations translate to
//!   `tools: [{type:"function", function:{name, description,
//!   parameters}}]`; assistant `ToolUse` content blocks translate to
//!   `messages[].tool_calls[]`; consumer-side `ToolResult` blocks
//!   translate to `role: "tool"` messages with `tool_call_id`.
//! - Server-Sent Events streaming: deltas → `TokenEventV2::Text`,
//!   tool-call deltas → buffered → `TokenEventV2::ToolUse` on
//!   `finish_reason: tool_calls`.
//! - Capabilities: advertises `v2 + tools`. `vision` / `audio` are
//!   off — the proto-side `Attachment` shape doesn't map cleanly to
//!   OpenAI's `image_url` (we'd need a base64 data URL, which is
//!   slow and provider-fragmented). Multimodal lands in a follow-up.
//!
//! # What we explicitly DO NOT do
//!
//! - No request retries (ADR 0007: caller owns retry).
//! - No mid-stream failover (ADR 0007: structurally broken).
//! - No HTTP server *inbound* (ADR 0006: outbound only).
//! - No SSE keepalive translation: if the upstream goes silent we
//!   surface that as a stream termination, not as a synthetic frame.
//! - No `Thinking` separation: OpenAI's surface doesn't expose a
//!   reasoning trace channel publicly. Providers that do (DeepSeek
//!   `reasoning_content`, OpenAI o1's hidden reasoning) need their
//!   own adapter or a follow-up patch.

mod adapter;
mod client;
mod mapper;

pub use adapter::{OpenAiCompat, OpenAiCompatConfig, OpenAiCompatError};
