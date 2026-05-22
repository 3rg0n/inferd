//! HTTP client + wire types for the OpenAI Chat Completions surface.
//!
//! Wire types follow the OpenAI public schema; provider-specific
//! extensions (`tool_calls`, `reasoning_content`, …) are accepted
//! through `serde(default)` so we don't reject novel fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request body for `POST /v1/chat/completions`.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// We always stream — non-streaming would require us to decode the
    /// full response then synthesize one Text frame, which is strictly
    /// worse than streaming for our consumers.
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDecl>,
    /// Always include usage in stream chunks. OpenAI's standard schema
    /// emits a final chunk with `usage: {...}` when this is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ChatMessage {
    pub role: String,
    /// String or array — provider-specific. We only emit string today;
    /// the wire keeps this generic so future multimodal can drop in
    /// `[{"type":"text","text":"..."}, {"type":"image_url",...}]`
    /// without a wire break.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallReplay>,
    /// Pairs a `role: "tool"` message with the assistant's prior call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool-call results are addressed by name (some providers care).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ToolCallReplay {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ToolCallFunction {
    pub name: String,
    /// Provider expects a JSON string here, not a JSON object.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ToolDecl {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolDeclFunction,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ToolDeclFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// --- Streaming response (SSE chunks) -------------------------------

/// One chunk off the SSE stream.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ChatChunk {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// OpenAI emits this once at the end of the stream when
    /// `stream_options.include_usage = true`.
    #[serde(default)]
    pub usage: Option<ChunkUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ChunkChoice {
    #[serde(default)]
    pub delta: ChunkDelta,
    /// `null` until the final chunk for this choice; one of `stop`,
    /// `length`, `tool_calls`, `content_filter`, `function_call` (deprecated).
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct ChunkDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ChunkToolCallDelta>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ChunkToolCallDelta {
    /// Index into the choice's tool_calls array. Required because
    /// deltas for different calls interleave.
    pub index: usize,
    /// Set on the first delta for this call; absent thereafter.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<ChunkToolCallFunctionDelta>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct ChunkToolCallFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ChunkUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
}
