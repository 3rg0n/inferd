//! v2 tool definitions and tool-call-id strong type.
//!
//! Per ADR 0015 §"v2 Tool definition". Mirrors Anthropic's tool-use
//! shape: name + free-form description + JSON Schema input
//! descriptor. The daemon's chat-templating layer wraps these into
//! whatever per-engine sequence the model expects (Gemma 4's
//! `<|tool_call>...<tool_call|>` sequence in the v0.2 default
//! adapter).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Strong type around the string id that pairs an `assistant`-emitted
/// `tool_use` block with the matching `tool_result` block in the
/// consumer's follow-up request. Wrapping it lets the daemon ensure
/// the round-trip uses the same id and lets middleware authors avoid
/// passing raw `String` for ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

impl ToolCallId {
    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ToolCallId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ToolCallId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Free-form JSON object representing a tool's invocation arguments.
///
/// Type-aliased to `serde_json::Value` deliberately: the daemon does
/// not enforce the consumer's `Tool::input_schema` (that's the
/// consumer's responsibility before executing the tool). The daemon
/// guarantees only that the model's emitted JSON parses; semantic
/// validation lives in the consumer.
pub type ToolUseInput = Value;

/// One tool definition in the request's top-level `tools[]` table.
///
/// `name` must be unique within a request. The model can emit a
/// `tool_use` content block referencing exactly this name; the
/// daemon's chat-templating layer is responsible for shaping the
/// definition into the format the active engine expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Caller-chosen name; unique within the enclosing request.
    pub name: String,
    /// Free-form description shown to the model so it can decide whether
    /// to call this tool.
    pub description: String,
    /// JSON Schema for the tool's input. The daemon does not enforce
    /// this; it's documentation for the model and validation surface
    /// for the consumer when the result comes back.
    pub input_schema: Value,
}
