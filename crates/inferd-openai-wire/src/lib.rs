//! OpenAI Chat Completions + Embeddings wire types.
//!
//! Pure serde structs, no logic. These describe the OpenAI public REST
//! schema and are shared by two directions so the wire has **one**
//! canonical definition and cannot drift:
//!
//! - **Outbound** ([`inferd-engine`]'s `openai_compat` adapter): inferd
//!   calls an upstream OpenAI-compatible provider — it *serializes*
//!   requests and *deserializes* response chunks.
//! - **Inbound** (the `inferd-http` bridge): an OpenAI-SDK client calls
//!   inferd — the bridge *deserializes* requests and *serializes*
//!   response chunks.
//!
//! Because both directions exist, every type derives **both**
//! `Serialize` and `Deserialize`. Provider-specific / novel fields are
//! accepted via `#[serde(default)]` so neither direction rejects
//! unknown input.
//!
//! `no_std`-free but dependency-light (serde + serde_json only).

#![warn(missing_docs, rust_2018_idioms)]
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ===================== Chat Completions =============================

/// Request body for `POST /v1/chat/completions`.
///
/// Outbound sets `stream = true` always; inbound honours the client's
/// `stream` (default `false` when omitted, per the OpenAI API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Upstream model identifier. Inbound accepts any value and echoes
    /// it back (inferd serves one warm model); outbound sends the
    /// configured upstream model.
    pub model: String,
    /// Conversation turns.
    pub messages: Vec<ChatMessage>,
    /// Stream SSE chunks vs. return one JSON body. Per the OpenAI API,
    /// an omitted `stream` means **non-streaming** (`false`) — the SDK
    /// relies on this, so the default must be `false`, not `true`.
    #[serde(default)]
    pub stream: bool,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling top-p.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Max tokens to generate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Number of choices. inferd generates one per request; the inbound
    /// bridge rejects `n > 1`. Optional; absent means 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Tool declarations the model may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDecl>,
    /// Ask the provider to include usage in the final stream chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    /// Structured-output constraint. The inbound bridge maps a
    /// `json_schema` form to the daemon's grammar-constrained decoding
    /// (ADR 0013); other forms are ignored (best-effort text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// `stream_options` object — only `include_usage` is used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamOptions {
    /// Emit a final chunk carrying token usage.
    pub include_usage: bool,
}

/// OpenAI `response_format`. Tagged by `type`. inferd's bridge honours
/// `json_schema` (mapped to the daemon's JSON-Schema grammar); `text`
/// (the implicit default) and `json_object` (schema-less JSON) carry no
/// schema to constrain against, so the bridge treats them as
/// unconstrained — the model is still asked for JSON via the prompt, but
/// no grammar is applied. Unknown types deserialize to `Other` and are
/// ignored rather than rejected (forward-compat).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Plain text (OpenAI's implicit default).
    Text,
    /// Schema-less JSON mode.
    JsonObject,
    /// JSON constrained to a schema. OpenAI nests it under a
    /// `json_schema` object: `{"type":"json_schema","json_schema":{
    /// "name":...,"schema":{...}}}`.
    JsonSchema {
        /// The `json_schema` object (name + schema).
        json_schema: JsonSchemaSpec,
    },
    /// Any `type` the bridge does not recognise — ignored.
    #[serde(other)]
    Other,
}

/// The `json_schema` object inside a `response_format`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchemaSpec {
    /// Caller-chosen name for the schema (required by the OpenAI API;
    /// inferd does not use it but accepts it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The JSON Schema the output must conform to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// OpenAI's strict flag; accepted and ignored (inferd's grammar is
    /// always strict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// One conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// `system` | `user` | `assistant` | `tool`.
    pub role: String,
    /// Message content: either a plain string or an array of typed
    /// parts (text + `image_url`), per the OpenAI Chat API. Absent for
    /// an assistant turn that carries only `tool_calls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// Assistant tool-call requests being replayed as history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallReplay>,
    /// On a `role: "tool"` message, the id of the assistant call it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional tool name (some providers key results by name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A message's `content` field. The OpenAI wire allows it to be *either*
/// a bare string *or* an array of typed content parts (text +
/// `image_url`). This untagged enum matches both forms and serializes
/// back to whichever shape it holds — a plain string round-trips as a
/// JSON string (so the outbound adapter's text-only messages are byte
/// -identical to before), an array round-trips as an array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// The common case: one text string.
    Text(String),
    /// Multimodal: an ordered list of text / image parts.
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// Borrow the string form when this content is a bare string.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Parts(_) => None,
        }
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

/// One element of a multimodal `content` array. Tagged by `type`;
/// `text`, `image_url` and `input_audio` are recognised (the three the
/// OpenAI SDK emits for chat). Unknown types deserialize into
/// [`ContentPart::Unknown`] so a newer client field doesn't hard-fail the
/// parse — the caller decides how to treat it (the bridge rejects it
/// explicitly rather than silently dropping content).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// A text span.
    Text {
        /// The text.
        text: String,
    },
    /// An image, carried as a URL (data: URL or remote URL).
    ImageUrl {
        /// The `image_url` object.
        image_url: ImageUrl,
    },
    /// An audio clip, carried inline as base64 (OpenAI defines no URL
    /// form for audio input).
    InputAudio {
        /// The `input_audio` object.
        input_audio: InputAudio,
    },
    /// Any part type the bridge does not recognise.
    #[serde(other)]
    Unknown,
}

/// The `image_url` object inside an `image_url` content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    /// Either a `data:<mime>;base64,<payload>` URL or a remote
    /// `http(s)://` URL. inferd's bridge accepts only `data:` URLs (a
    /// server-side fetch of an arbitrary remote URL is an SSRF vector);
    /// remote URLs are rejected with a clear 400.
    pub url: String,
    /// OpenAI's fidelity hint (`auto` | `low` | `high`). Accepted and
    /// ignored — inferd's image budget is an operator/model property
    /// (`mmproj_image_max_tokens`), not a per-request knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The `input_audio` object inside an `input_audio` content part.
///
/// OpenAI carries audio input inline only — there is no `audio_url`
/// form — so `data` is always raw standard base64 (no `data:` URL
/// prefix). The bridge decodes it, downmixes to mono and resamples to
/// the rate the daemon's backend requires (ADR 0025).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputAudio {
    /// Base64-encoded audio bytes (standard alphabet, no `data:` prefix).
    pub data: String,
    /// Container/codec hint: `"wav"` or `"mp3"` on the OpenAI wire. Used
    /// as a probe hint only — the actual format is detected from the
    /// bytes, so a wrong hint costs nothing.
    pub format: String,
}

/// An assistant tool call, as replayed in message history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallReplay {
    /// Correlates with the matching `role: "tool"` result.
    pub id: String,
    /// Always `"function"` today.
    #[serde(rename = "type")]
    pub kind: String,
    /// The called function + its argument payload.
    pub function: ToolCallFunction,
}

/// A tool call's function name + JSON-string arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    /// Function name.
    pub name: String,
    /// Arguments as a JSON **string** (not an object), per the OpenAI wire.
    pub arguments: String,
}

/// A tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDecl {
    /// Always `"function"` today.
    #[serde(rename = "type")]
    pub kind: String,
    /// The function schema.
    pub function: ToolDeclFunction,
}

/// A tool's declared name / description / JSON-Schema parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDeclFunction {
    /// Function name.
    pub name: String,
    /// Human/model-facing description.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: Value,
}

// --- Streaming response (SSE chunks) --------------------------------

/// One `chat.completion.chunk` off the SSE stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatChunk {
    /// Stable id for the completion (same across all chunks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `"chat.completion.chunk"` (stream) / `"chat.completion"` (non-stream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// Unix creation timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
    /// Echoed model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Per-choice deltas (inferd emits a single choice).
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// Token usage, present on the final chunk when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChunkUsage>,
}

/// One choice's delta + finish reason within a chunk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// Choice index (always 0 for inferd).
    #[serde(default)]
    pub index: u32,
    /// Incremental content / tool-call delta for this chunk.
    #[serde(default)]
    pub delta: ChunkDelta,
    /// `null` until the final chunk: `stop` | `length` | `tool_calls` | …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// The incremental payload in a streaming choice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkDelta {
    /// Assistant role, sent on the first delta of a stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Text delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool-call deltas (interleaved by `index`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ChunkToolCallDelta>,
}

/// A streamed tool-call fragment, keyed by `index`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkToolCallDelta {
    /// Index into the choice's tool_calls array (deltas interleave).
    pub index: usize,
    /// Set on the first fragment for this call; absent thereafter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `"function"` on the first fragment.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    /// The function name/arguments fragment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<ChunkToolCallFunctionDelta>,
}

/// Function name / arguments fragment inside a tool-call delta.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkToolCallFunctionDelta {
    /// Function name (first fragment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Argument JSON-string fragment (accumulates across fragments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// Token usage on the terminal chunk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkUsage {
    /// Prompt (input) tokens.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Completion (output) tokens.
    #[serde(default)]
    pub completion_tokens: u32,
    /// prompt + completion.
    #[serde(default)]
    pub total_tokens: u32,
}

// ===================== Embeddings ===================================

/// Request body for `POST /v1/embeddings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsRequest {
    /// Model identifier (echoed back; inferd serves one embed model).
    pub model: String,
    /// A single string or an array of strings. Kept as a raw `Value` so
    /// the handler coerces both shapes.
    pub input: Value,
    /// Matryoshka truncation dimension (ADR 0017 MRL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
    /// `float` | `base64`. inferd emits float32; base64 is not supported
    /// in v1 (the bridge errors if `base64` is requested).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
}

/// `POST /v1/embeddings` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsResponse {
    /// Always `"list"`.
    pub object: String,
    /// One entry per input string.
    pub data: Vec<EmbeddingData>,
    /// Echoed model name.
    pub model: String,
    /// Token usage.
    pub usage: EmbeddingsUsage,
}

/// One embedding vector + its index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingData {
    /// Always `"embedding"`.
    pub object: String,
    /// Position in the input array.
    pub index: usize,
    /// The embedding, encoded per the request's `encoding_format`:
    /// a JSON float array (`float`) or a base64 string (`base64`, the
    /// OpenAI SDK's default).
    pub embedding: EmbeddingVector,
}

/// An embedding vector serialized either as a float array or a base64
/// string, chosen to match the client's requested `encoding_format`.
/// Untagged: a float array becomes a JSON array; base64 becomes a JSON
/// string — exactly the two shapes OpenAI emits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingVector {
    /// `encoding_format: "float"` — a JSON array of f32.
    Floats(Vec<f32>),
    /// `encoding_format: "base64"` — base64 of the little-endian f32 bytes.
    Base64(String),
}

/// Token usage for an embeddings response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbeddingsUsage {
    /// Input tokens.
    pub prompt_tokens: u32,
    /// Same as prompt_tokens for embeddings.
    pub total_tokens: u32,
}

// ===================== Errors =======================================

/// OpenAI error envelope: `{"error": {...}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    /// The error detail.
    pub error: ErrorBody,
}

/// The error object OpenAI clients expect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Human-readable message.
    pub message: String,
    /// Error class, e.g. `invalid_request_error`, `rate_limit_error`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Optional machine code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}
