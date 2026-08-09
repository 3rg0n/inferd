//! v2 response frame schema.
//!
//! Per ADR 0015 §"v2 Response frames". Three frame variants:
//!
//! - `frame` — streaming partial output. Carries one `ResponseBlock`,
//!   which is either a `text` delta (incremental token text), a
//!   `thinking` delta (reasoning trace, separated from user-visible
//!   output per `docs/thinking.mode.in.gemma.md`), or a complete
//!   `tool_use` block (the daemon parses Gemma 4's
//!   `<|tool_call>...<tool_call|>` sequence in full before emitting).
//!
//! - `done` — terminal success frame. Carries usage and the v2
//!   stop-reason taxonomy (`end_turn`, `max_tokens`, `tool_use`,
//!   `stop_sequence`).
//!
//! - `error` — terminal failure frame. Carries the v2 error-code
//!   taxonomy (v1 codes plus `attachment_unsupported` and
//!   `tool_call_malformed`).

use crate::v2::tool::{ToolCallId, ToolUseInput};
use serde::{Deserialize, Serialize};

/// Why a v2 generation ended. Carried on `done` frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReasonV2 {
    /// Model emitted the end-of-turn token cleanly. Equivalent to v1's `end`.
    EndTurn,
    /// `max_tokens` reached.
    MaxTokens,
    /// Model emitted a complete `tool_use` block; consumer must
    /// execute the tool and send a follow-up request with a
    /// `tool_result` content block.
    ToolUse,
    /// Generation hit a configured stop sequence (reserved for v2.x;
    /// v2.0 daemons may not emit this).
    StopSequence,
    /// Caller disconnected or otherwise cancelled.
    Cancelled,
    /// Generation aborted; partial output was already emitted.
    Error,
}

/// Token-count usage report carried on v2 `done` frames.
///
/// Field names (`input_tokens` / `output_tokens`) match Anthropic's
/// shape; v1's `prompt_tokens` / `completion_tokens` matched
/// llama.cpp's terminology. Both refer to the same underlying counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageV2 {
    /// Tokens consumed by the prompt (including templated chat structure).
    pub input_tokens: u32,
    /// Tokens generated in the response.
    pub output_tokens: u32,
}

/// One streaming-output payload carried inside a `frame` response.
///
/// Variants:
/// - `Text { delta }` — incremental token text. Multiple `Text` frames
///   per request are normal; concatenating their `delta`s yields the
///   full response text.
/// - `Thinking { delta }` — incremental reasoning-trace text. The
///   daemon separates content emitted between Gemma 4's `<|think|>`
///   and `<|/think|>` tokens into this variant so middleware can
///   choose to display, hide, or log it independently.
/// - `ToolUse { ... }` — a complete tool-call request from the model.
///   Arrives whole, not streamed. Following a `ToolUse` block, the
///   stream typically terminates with `Done { stop_reason: ToolUse }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseBlock {
    /// Incremental user-visible text.
    Text {
        /// Newly-generated text since the last `Text` frame.
        delta: String,
    },
    /// Incremental reasoning-trace text.
    Thinking {
        /// Newly-generated reasoning trace since the last `Thinking` frame.
        delta: String,
    },
    /// Complete model-emitted tool-call request.
    ToolUse {
        /// Pairs this invocation with the consumer's eventual
        /// `ToolResult` block in the next request.
        tool_call_id: ToolCallId,
        /// Tool name; matches a `Tool::name` from the request's
        /// `tools[]` table.
        name: String,
        /// JSON arguments emitted by the model.
        input: ToolUseInput,
    },
}

/// v2 error-code taxonomy. Superset of v1's `ErrorCode` (kept
/// independent so the v1 enum stays frozen per ADR 0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCodeV2 {
    /// Admission queue full at submit time.
    QueueFull,
    /// Selected backend errored before or during generation.
    BackendUnavailable,
    /// Request failed validation (bad shape, dangling attachment id, etc.).
    InvalidRequest,
    /// Frame exceeded the 64 MiB cap.
    FrameTooLarge,
    /// Daemon-side bug or unexpected condition.
    Internal,
    /// Backend cannot handle the requested attachment kind / MIME.
    AttachmentUnsupported,
    /// Model emitted a tool-call sequence the daemon couldn't parse.
    ToolCallMalformed,
    /// Request's `wire_version` is not one this daemon supports
    /// (ADR 0021). The error `message` names both the requested and
    /// supported versions so the mismatch is debuggable.
    WireVersionUnsupported,
}

/// `skip_serializing_if` predicate for a `bool` that defaults to
/// `false`. Keeps an unset optional flag off the wire entirely rather
/// than emitting `false`, so an older client sees the frame shape it
/// already parses.
fn is_false(b: &bool) -> bool {
    !*b
}

/// One frame on the v2 response NDJSON stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseV2 {
    /// One streaming-output payload.
    Frame {
        /// Request id.
        id: String,
        /// The payload — text delta, thinking delta, or complete tool_use.
        block: ResponseBlock,
    },
    /// Terminal frame for a successful generation.
    Done {
        /// Request id.
        id: String,
        /// Token-count usage.
        usage: UsageV2,
        /// Why generation stopped.
        stop_reason: StopReasonV2,
        /// `Backend::name()` of the adapter that served this request.
        ///
        /// Diagnostic only — apps must not branch on this (ADR 0007).
        backend: String,
        /// Set when the request carried `tool_choice: "required"` and
        /// the generation ended **without** the model ever emitting a
        /// tool call. Omitted otherwise, so a v0.7.0 client parses this
        /// frame unchanged.
        ///
        /// `required` guarantees the turn cannot *end* without a call —
        /// the grammar masks every end-of-turn token while the call rule
        /// is unsatisfied — but it cannot guarantee a call *arrives*:
        /// unlimited non-call text is legal before one, so a model that
        /// disagrees with the prompt can decline until `max_tokens`
        /// (ADR 0029, measured on Gemma 4 E4B). Without this field the
        /// caller sees `stop_reason: "max_tokens"` and cannot tell
        /// "ran out of room mid-answer" from "declined for the whole
        /// budget" — the same request shape produces both.
        ///
        /// A new `stop_reason` variant would have said this more
        /// directly, but [`StopReasonV2`] is closed on both sides of
        /// the wire (no `#[serde(other)]` here, fixed string constants
        /// in `clients/go`), so an unfamiliar value is a parse error,
        /// not a graceful degrade. An optional field is the only shape
        /// that adds the signal without breaking a deployed client.
        ///
        /// So: `true` means no call arrived and retrying is the caller's
        /// call. It is a *report*, never a repair — the daemon does not
        /// retry, reshape, or nudge the model (invariant #2).
        #[serde(default, skip_serializing_if = "is_false")]
        tool_choice_unsatisfied: bool,
    },
    /// Terminal frame for a failed generation.
    Error {
        /// Request id.
        id: String,
        /// Machine-readable classification.
        code: ErrorCodeV2,
        /// Human-readable description.
        message: String,
    },
}

impl ResponseV2 {
    /// Correlation id of the frame regardless of variant.
    pub fn id(&self) -> &str {
        match self {
            ResponseV2::Frame { id, .. }
            | ResponseV2::Done { id, .. }
            | ResponseV2::Error { id, .. } => id,
        }
    }

    /// `true` if this frame ends a request stream.
    pub fn is_terminal(&self) -> bool {
        matches!(self, ResponseV2::Done { .. } | ResponseV2::Error { .. })
    }
}
