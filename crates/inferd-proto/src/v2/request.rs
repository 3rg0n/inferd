//! v2 request envelope, message + content-block types, and validation.
//!
//! Per ADR 0015 §"v2 Request" + §"v2 ContentBlock variants". The
//! shape mirrors Anthropic's `/v1/messages` envelope (typed content
//! blocks, top-level attachments[] table, top-level tools[] table)
//! with HTTP stripped and inferd-specific fields (`id`) added.

use crate::error::ProtoError;
use crate::v2::attachment::{Attachment, MAX_ATTACHMENTS_PER_REQUEST};
use crate::v2::tool::{Tool, ToolCallId, ToolUseInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Structured output format constraint for generation.
///
/// Specifies a JSON Schema that the model output must conform to.
/// The daemon translates this to engine-specific constraints (e.g., GBNF
/// grammar for llamacpp). Backends that don't support structured output
/// ignore this field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Output must conform to a JSON Schema.
    JsonSchema {
        /// The JSON Schema that output must match.
        schema: Value,
    },
}

/// How hard the daemon should push the model toward calling a tool.
///
/// Serialised as a bare JSON string (`"auto"` / `"required"` /
/// `"none"`), matching how OpenAI and Anthropic spell the same three
/// modes, so a bridge maps it without inventing an envelope.
///
/// This is a *constraint*, not a hint, on backends that advertise
/// `tools` support: the llamacpp backend compiles the loaded family's
/// tool-call syntax to a GBNF grammar and installs it on the sampler,
/// so `Required` cannot come back as prose. Backends whose upstream
/// has its own equivalent (the cloud adapters) forward it; a backend
/// that can express neither rejects the request rather than silently
/// downgrading to a hint — a fail-open `tool_choice` is worse than an
/// absent one, because the caller believes it holds a guarantee.
///
/// Forward-compatibility: an unrecognised value deserialises to
/// [`ToolChoice::Unknown`] rather than failing the parse, and the
/// daemon rejects it explicitly. Naming a *specific* tool (OpenAI's
/// `{"type":"function","function":{"name":…}}`) is deliberately not
/// modelled yet; it stays additive because this enum is non-exhaustive
/// in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    /// The model decides. Equivalent to omitting the field, except the
    /// grammar (when the backend installs one) additionally constrains
    /// the *shape* of a call the model chooses to make.
    Auto,
    /// The model must emit at least one tool call. On llamacpp this is
    /// a non-lazy grammar whose root requires one, so no path through
    /// sampling produces a bare text answer.
    Required,
    /// The model must not call a tool. Tool declarations still reach
    /// the prompt (removing them would change the rendered context),
    /// but no grammar is installed and the daemon does not shape the
    /// output toward a call.
    None,
    /// Any value this build does not recognise. Kept so a newer
    /// client's request parses; the daemon then rejects it with
    /// `invalid_request` rather than guessing which of the three modes
    /// was meant.
    #[serde(other)]
    Unknown,
}

/// Conversation role on a v2 message.
///
/// Same set as v1's `Role` (system / user / assistant) but defined
/// independently so v1 and v2 can evolve their role enums without
/// affecting each other. Tool roles are *not* a separate
/// conversation-role variant in v2: a tool invocation is an
/// `assistant`-role message containing a `tool_use` content block,
/// and the result is a `user`-role message containing a
/// `tool_result` content block. This matches Anthropic's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleV2 {
    /// System prompt setting overall instructions.
    System,
    /// End-user input (or tool results, dressed as user-role).
    User,
    /// Prior model output, including tool-use requests.
    Assistant,
}

/// One element of a `MessageV2::content` array.
///
/// Forward-compatibility: unknown content-block types deserialise as
/// the `Unknown` variant so v2.0 daemons / clients ignore content
/// shapes added in later v2.x revisions gracefully. The daemon emits
/// `invalid_request` only if the model needs the unknown content to
/// proceed (per ADR 0015 §"v2 ContentBlock variants").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text segment within a message.
    Text {
        /// Inline text. May be empty (rare but legal).
        text: String,
    },
    /// Reference to an `image`-kind attachment in the request's top-level
    /// `attachments[]` table.
    Image {
        /// Must match exactly one `Attachment::id` of kind `Image`.
        attachment_id: String,
    },
    /// Reference to an `audio`-kind attachment.
    Audio {
        /// Must match exactly one `Attachment::id` of kind `Audio`.
        attachment_id: String,
    },
    /// Reference to a `video`-kind attachment. Backends that don't
    /// support video reject the request with `attachment_unsupported`.
    Video {
        /// Must match exactly one `Attachment::id` of kind `Video`.
        attachment_id: String,
    },
    /// Assistant-emitted invocation. Consumers don't typically construct
    /// these on the request side — the daemon emits them as response
    /// frames; consumers then send a follow-up request with a matching
    /// `ToolResult` block. Allowed in request `messages[]` only when
    /// replaying prior assistant turns for context.
    ToolUse {
        /// Pairs this invocation with the corresponding `ToolResult`.
        tool_call_id: ToolCallId,
        /// Tool name, must match a `Tool::name` from the request's
        /// `tools[]` table (or a tool the model knows from training).
        name: String,
        /// JSON arguments emitted by the model.
        input: ToolUseInput,
    },
    /// Consumer-constructed result of executing a tool. Routed back into
    /// the model's context by the daemon's chat-templating layer.
    ToolResult {
        /// Must match the `tool_call_id` of the assistant-emitted
        /// `ToolUse` block this is responding to.
        tool_call_id: ToolCallId,
        /// Result content; typically a single `Text` block.
        content: Vec<ContentBlock>,
    },
    /// Forward-compatible escape hatch — any `type` value the local
    /// build doesn't recognise lands here so older clients/daemons
    /// don't reject newer payloads at parse time.
    #[serde(other)]
    Unknown,
}

/// One message in the v2 conversation history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageV2 {
    /// Speaker.
    pub role: RoleV2,
    /// Typed content blocks; must be non-empty.
    pub content: Vec<ContentBlock>,
}

/// The v2 request envelope sent by clients.
///
/// `Default` is intentionally available for `..Default::default()`
/// shorthand; callers must populate `id` and `messages` before
/// sending.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RequestV2 {
    /// Wire-format version the client speaks (ADR 0021). Defaults to 0
    /// on deserialise so a frame that omits it is treated as the
    /// pre-v0.4 framing and rejected by a v0.4 daemon with a clear
    /// `wire_version_unsupported` error. Clients set this to
    /// [`crate::v2::WIRE_VERSION`].
    #[serde(default)]
    pub wire_version: u32,

    /// Caller-assigned correlation id; echoed on every response frame.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,

    /// Conversation history in chronological order. Must be non-empty.
    pub messages: Vec<MessageV2>,

    /// Binary attachments referenced by `attachment_id` from content
    /// blocks. Empty when the request is text-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,

    /// Tool definitions the model may call. Empty when no tools are
    /// in scope for this request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    /// How hard to push the model toward calling one of `tools`.
    /// Absent means [`ToolChoice::Auto`]'s *behaviour* without the
    /// grammar — i.e. exactly what pre-v0.8 clients got — so omitting
    /// it is behaviour-preserving. Meaningless without `tools`, and the
    /// daemon rejects that combination rather than ignoring the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Sampling temperature; daemon applies engine default if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Nucleus sampling probability; daemon applies engine default if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// Top-k sampling cutoff; daemon applies engine default if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// Maximum tokens to generate; daemon applies engine default if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Stream tokens vs return one final `done`; daemon defaults to streaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// Optional structured output constraint; daemon ignores if backend doesn't support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,

    /// Request reasoning ("thinking") mode. When `Some(true)`, the daemon
    /// asks the model to produce an internal reasoning trace before its
    /// answer; that trace is separated onto `thinking` response blocks
    /// (it does not leak into user-visible `text`). `None`/`Some(false)`
    /// = no thinking (default, behaviour-preserving). The daemon shapes
    /// this per engine (for llamacpp/Gemma 4 it injects the `<|think|>`
    /// activation token into the system turn, ADR 0013); backends that
    /// don't support reasoning ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
}

/// `RequestV2` with semantic validation completed.
///
/// Differences from `RequestV2`: attachment ids referenced from
/// content blocks are guaranteed to resolve; tool names referenced
/// from `ToolUse` blocks are guaranteed to be unique within the
/// `tools[]` table; sampling fields stay `Option` here (engine
/// defaults are applied at the backend layer, not the proto layer,
/// because they vary per backend in v2 — unlike v1 where Gemma 4
/// defaults could be hard-coded).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedV2 {
    /// Wire-format version the request declared (already validated as
    /// supported by `resolve`).
    pub wire_version: u32,
    /// Caller-assigned correlation id.
    pub id: String,
    /// Validated conversation history.
    pub messages: Vec<MessageV2>,
    /// Validated attachment table.
    pub attachments: Vec<Attachment>,
    /// Validated tool definitions.
    pub tools: Vec<Tool>,
    /// Tool-choice constraint, if set. `resolve` has already rejected
    /// [`ToolChoice::Unknown`] and any value sent without `tools`, so a
    /// backend reading this can trust it names one of the three modes
    /// and that `tools` is non-empty.
    pub tool_choice: Option<ToolChoice>,
    /// Sampling temperature, if set.
    pub temperature: Option<f64>,
    /// Nucleus sampling probability, if set.
    pub top_p: Option<f64>,
    /// Top-k cutoff, if set.
    pub top_k: Option<u32>,
    /// Max tokens, if set.
    pub max_tokens: Option<u32>,
    /// Streaming flag, if set.
    pub stream: Option<bool>,
    /// Structured output constraint, if set.
    pub response_format: Option<ResponseFormat>,
    /// Reasoning ("thinking") mode request, if set.
    pub thinking: Option<bool>,
}

impl RequestV2 {
    /// Validate the request envelope. Resolves attachment-id references,
    /// checks uniqueness of attachment ids and tool names, and
    /// rejects empty `messages` / empty `content` arrays.
    ///
    /// Does NOT apply sampling defaults — those are backend-specific
    /// in v2 (cloud backends and llamacpp pick different sensible
    /// defaults). Backends fill them in at `generate_v2` time.
    pub fn resolve(self) -> Result<ResolvedV2, ProtoError> {
        if self.messages.is_empty() {
            return Err(ProtoError::InvalidRequest(
                "messages must not be empty".into(),
            ));
        }

        // Bound the attachment table before anything else touches it: each
        // entry entitles the sender to one more 64 MiB BLOB frame, so an
        // unbounded table is a read-amplification lever (THREAT_MODEL F-1).
        // Readers enforce the same cap while streaming, before the bytes
        // arrive; this is the contract restated where every producer and
        // every non-streaming consumer also sees it.
        if self.attachments.len() > MAX_ATTACHMENTS_PER_REQUEST {
            return Err(ProtoError::InvalidRequest(format!(
                "request declares {} attachments; at most {MAX_ATTACHMENTS_PER_REQUEST} allowed",
                self.attachments.len()
            )));
        }

        let mut attachments_by_id: HashMap<&str, &Attachment> = HashMap::new();
        for att in &self.attachments {
            if matches!(att, Attachment::Unknown) {
                return Err(ProtoError::InvalidRequest(
                    "attachments contain an unknown kind".into(),
                ));
            }
            let id = att.id();
            if id.is_empty() {
                return Err(ProtoError::InvalidRequest(
                    "attachments must have non-empty id".into(),
                ));
            }
            if attachments_by_id.insert(id, att).is_some() {
                return Err(ProtoError::InvalidRequest(format!(
                    "duplicate attachment id: {id}"
                )));
            }
        }

        let mut tool_names: HashSet<&str> = HashSet::new();
        for tool in &self.tools {
            if !tool_names.insert(tool.name.as_str()) {
                return Err(ProtoError::InvalidRequest(format!(
                    "duplicate tool name: {}",
                    tool.name
                )));
            }
        }

        // `tool_choice` is a constraint on `tools`, so it is only
        // meaningful alongside them, and an unrecognised mode must not
        // be silently treated as `auto`. Both cases are rejected here
        // rather than dropped: a caller who asked for `required` and
        // got best-effort text has been handed a false guarantee, which
        // is the fail-open failure this field exists to close.
        match self.tool_choice {
            Some(ToolChoice::Unknown) => {
                return Err(ProtoError::InvalidRequest(
                    "tool_choice must be one of: auto, required, none".into(),
                ));
            }
            Some(_) if self.tools.is_empty() => {
                return Err(ProtoError::InvalidRequest(
                    "tool_choice requires a non-empty tools array".into(),
                ));
            }
            _ => {}
        }

        for (mi, msg) in self.messages.iter().enumerate() {
            if msg.content.is_empty() {
                return Err(ProtoError::InvalidRequest(format!(
                    "messages[{mi}].content must not be empty"
                )));
            }
            validate_content_blocks(&msg.content, mi, &attachments_by_id, &tool_names)?;
        }

        // Note: `wire_version` is carried through unchecked here.
        // Enforcing "which versions this daemon accepts" is a daemon
        // policy (it advertises its supported version in the
        // capabilities frame), so the daemon checks `wire_version`
        // against `crate::v2::WIRE_VERSION` and emits
        // `ErrorCodeV2::WireVersionUnsupported` before dispatch —
        // proto stays policy-free.
        Ok(ResolvedV2 {
            wire_version: self.wire_version,
            id: self.id,
            messages: self.messages,
            attachments: self.attachments,
            tools: self.tools,
            tool_choice: self.tool_choice,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            max_tokens: self.max_tokens,
            stream: self.stream,
            response_format: self.response_format,
            thinking: self.thinking,
        })
    }
}

fn validate_content_blocks(
    blocks: &[ContentBlock],
    msg_index: usize,
    attachments_by_id: &HashMap<&str, &Attachment>,
    tool_names: &HashSet<&str>,
) -> Result<(), ProtoError> {
    for (bi, block) in blocks.iter().enumerate() {
        match block {
            ContentBlock::Text { .. } => {}
            ContentBlock::Image { attachment_id } => check_kind(
                msg_index,
                bi,
                attachment_id,
                attachments_by_id,
                Attachment::is_image,
                "image",
            )?,
            ContentBlock::Audio { attachment_id } => check_kind(
                msg_index,
                bi,
                attachment_id,
                attachments_by_id,
                Attachment::is_audio,
                "audio",
            )?,
            ContentBlock::Video { attachment_id } => check_kind(
                msg_index,
                bi,
                attachment_id,
                attachments_by_id,
                Attachment::is_video,
                "video",
            )?,
            ContentBlock::ToolUse { name, .. } => {
                // tool_names may be empty if the request replays an
                // assistant message that references a tool the model
                // knew from training but the consumer didn't redeclare.
                // We do not reject here.
                let _ = (name, tool_names);
            }
            ContentBlock::ToolResult { content, .. } => {
                // Recurse — tool_result wraps further content blocks.
                validate_content_blocks(content, msg_index, attachments_by_id, tool_names)?;
            }
            ContentBlock::Unknown => {
                return Err(ProtoError::InvalidRequest(format!(
                    "messages[{msg_index}].content[{bi}] uses unknown content-block type"
                )));
            }
        }
    }
    Ok(())
}

fn check_kind(
    msg_index: usize,
    block_index: usize,
    attachment_id: &str,
    attachments_by_id: &HashMap<&str, &Attachment>,
    pred: fn(&Attachment) -> bool,
    expected: &str,
) -> Result<(), ProtoError> {
    let att = attachments_by_id.get(attachment_id).ok_or_else(|| {
        ProtoError::InvalidRequest(format!(
            "messages[{msg_index}].content[{block_index}] references unknown attachment_id {attachment_id:?}"
        ))
    })?;
    if !pred(att) {
        return Err(ProtoError::InvalidRequest(format!(
            "messages[{msg_index}].content[{block_index}] block expects {expected} attachment but {attachment_id:?} is a different kind"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_format_round_trip() {
        // Test that response_format serializes and deserializes correctly.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        });

        let format = ResponseFormat::JsonSchema { schema };

        // Serialize to JSON.
        let json_str = serde_json::to_string(&format).expect("serialize");

        // Deserialize back.
        let deserialized: ResponseFormat = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(format, deserialized);
    }

    #[test]
    fn test_request_v2_with_response_format() {
        // Test that a RequestV2 with response_format round-trips correctly.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });

        let req = RequestV2 {
            wire_version: 1,
            id: "test".to_owned(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_owned(),
                }],
            }],
            response_format: Some(ResponseFormat::JsonSchema { schema }),
            ..Default::default()
        };

        // Serialize and deserialize.
        let json_str = serde_json::to_string(&req).expect("serialize");
        let deserialized: RequestV2 = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(req, deserialized);
        assert!(deserialized.response_format.is_some());
    }

    #[test]
    fn test_request_v2_without_response_format() {
        // Test that a RequestV2 without response_format is forward-compatible.
        let req = RequestV2 {
            wire_version: 1,
            id: "test".to_owned(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_owned(),
                }],
            }],
            ..Default::default()
        };

        // Serialize and deserialize.
        let json_str = serde_json::to_string(&req).expect("serialize");
        let deserialized: RequestV2 = serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(req, deserialized);
        assert!(deserialized.response_format.is_none());
    }

    fn tool() -> Tool {
        Tool {
            name: "get_weather".to_owned(),
            description: "look up the weather".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn req_with(tools: Vec<Tool>, tool_choice: Option<ToolChoice>) -> RequestV2 {
        RequestV2 {
            wire_version: 1,
            id: "tc".to_owned(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "weather?".to_owned(),
                }],
            }],
            tools,
            tool_choice,
            ..Default::default()
        }
    }

    /// The wire spelling is a bare string, not a tagged object — a
    /// bridge maps OpenAI's `"required"` straight through, and this is
    /// the frozen shape, so pin it rather than only round-tripping.
    #[test]
    fn tool_choice_serialises_as_a_bare_string() {
        for (choice, want) in [
            (ToolChoice::Auto, "\"auto\""),
            (ToolChoice::Required, "\"required\""),
            (ToolChoice::None, "\"none\""),
        ] {
            let got = serde_json::to_string(&choice).expect("serialize");
            assert_eq!(got, want);
            let back: ToolChoice = serde_json::from_str(&got).expect("deserialize");
            assert_eq!(back, choice);
        }
    }

    #[test]
    fn tool_choice_round_trips_on_the_envelope() {
        let req = req_with(vec![tool()], Some(ToolChoice::Required));
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(
            json.contains(r#""tool_choice":"required""#),
            "envelope: {json}"
        );
        let back: RequestV2 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(req, back);
    }

    /// Absent `tool_choice` must stay absent on the wire: a v0.7 daemon
    /// reading a v0.8 client's text-only frame sees no new key at all.
    #[test]
    fn absent_tool_choice_is_not_serialised() {
        let req = req_with(Vec::new(), None);
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(!json.contains("tool_choice"), "envelope: {json}");
        let back: RequestV2 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.tool_choice, None);
        assert_eq!(back.resolve().expect("resolves").tool_choice, None);
    }

    /// Forward-compat: a newer client's mode must parse (so the frame is
    /// readable) but must NOT resolve — silently treating an unknown
    /// mode as `auto` would answer a `required`-shaped request with
    /// best-effort prose.
    #[test]
    fn unknown_tool_choice_parses_but_is_rejected() {
        let json = r#"{"wire_version":1,"id":"tc","messages":[{"role":"user",
            "content":[{"type":"text","text":"hi"}]}],
            "tools":[{"name":"t","description":"d","input_schema":{}}],
            "tool_choice":"tool_named_foo"}"#;
        let req: RequestV2 = serde_json::from_str(json).expect("must parse");
        assert_eq!(req.tool_choice, Some(ToolChoice::Unknown));

        let err = req.resolve().expect_err("must not resolve");
        assert!(
            matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("tool_choice")),
            "unexpected error: {err}"
        );
    }

    /// `tool_choice` constrains `tools`; without them there is nothing
    /// to constrain, and honouring it would be a no-op the caller reads
    /// as enforcement.
    #[test]
    fn tool_choice_without_tools_is_rejected() {
        for choice in [ToolChoice::Auto, ToolChoice::Required, ToolChoice::None] {
            let err = req_with(Vec::new(), Some(choice))
                .resolve()
                .expect_err("must reject tool_choice with no tools");
            assert!(
                matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("tools")),
                "unexpected error for {choice:?}: {err}"
            );
        }
    }

    #[test]
    fn tool_choice_with_tools_resolves_and_carries_through() {
        for choice in [ToolChoice::Auto, ToolChoice::Required, ToolChoice::None] {
            let resolved = req_with(vec![tool()], Some(choice))
                .resolve()
                .expect("must resolve");
            assert_eq!(resolved.tool_choice, Some(choice));
        }
    }

    /// THREAT_MODEL F-1: the 64 MiB frame cap bounds one *frame*, not one
    /// *request*. Without a count cap, a single in-cap request frame can
    /// declare enough attachment entries to entitle the sender to
    /// arbitrarily many further 64 MiB BLOB reads.
    #[test]
    fn resolve_rejects_more_attachments_than_the_cap() {
        let n = MAX_ATTACHMENTS_PER_REQUEST + 1;
        let req = RequestV2 {
            wire_version: 1,
            id: "flood".to_owned(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_owned(),
                }],
            }],
            attachments: (0..n)
                .map(|i| Attachment::Image {
                    id: format!("a{i}"),
                    width: 1,
                    height: 1,
                    bytes: Vec::new(),
                })
                .collect(),
            ..Default::default()
        };

        let err = req.resolve().expect_err("must reject over-cap attachments");
        assert!(
            matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("attachments")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_accepts_attachments_at_the_cap() {
        let req = RequestV2 {
            wire_version: 1,
            id: "at-cap".to_owned(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_owned(),
                }],
            }],
            attachments: (0..MAX_ATTACHMENTS_PER_REQUEST)
                .map(|i| Attachment::Image {
                    id: format!("a{i}"),
                    width: 1,
                    height: 1,
                    bytes: Vec::new(),
                })
                .collect(),
            ..Default::default()
        };

        let resolved = req.resolve().expect("at-cap request must resolve");
        assert_eq!(resolved.attachments.len(), MAX_ATTACHMENTS_PER_REQUEST);
    }
}
