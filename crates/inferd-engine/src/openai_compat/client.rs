//! HTTP client + wire types for the OpenAI Chat Completions surface.
//!
//! The wire types themselves now live in the shared
//! [`inferd_openai_wire`] crate so the outbound adapter here and the
//! inbound `inferd-http` bridge share **one** canonical definition and
//! cannot drift. This module re-exports the subset the outbound adapter
//! uses; the HTTP-calling logic (reqwest) stays in `adapter.rs`.

pub(super) use inferd_openai_wire::{
    ChatChunk, ChatMessage, ChatRequest, ChunkToolCallDelta, StreamOptions, ToolCallFunction,
    ToolCallReplay, ToolChoice as WireToolChoice, ToolChoiceMode, ToolDecl, ToolDeclFunction,
};
