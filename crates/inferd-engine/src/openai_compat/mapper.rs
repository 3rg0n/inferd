//! `ResolvedV2` ⇄ OpenAI Chat Completions wire mapping.
//!
//! Two halves:
//!
//! - [`request_from_resolved`] turns a `ResolvedV2` envelope into the
//!   `ChatRequest` we POST to `/v1/chat/completions`. Text content
//!   blocks become `messages[].content`; assistant `ToolUse` blocks
//!   become `messages[].tool_calls[]`; consumer `ToolResult` blocks
//!   become a follow-up `role: "tool"` message addressed by
//!   `tool_call_id`. Tools translate to the `tools[]` table.
//!
//! - [`ChunkAccumulator`] absorbs SSE chunks (`ChatChunk`) and emits
//!   zero or more `TokenEventV2` values per chunk. Text deltas pass
//!   straight through; tool-call deltas accumulate per `index` until
//!   the choice's `finish_reason` arrives, at which point the buffered
//!   call is emitted as a `TokenEventV2::ToolUse`. Final `usage`
//!   carries the v2 `Done` frame.
//!
//! What we deliberately don't translate:
//!
//! - **Image / audio / video content blocks**. The OpenAI Chat
//!   Completions surface accepts `image_url` blocks but the mapping
//!   from inferd's raw-bytes attachment model (ADR 0016) to a base64
//!   data URL is provider-fragmented and slow. Multimodal lands in a
//!   later phase. For v0.2, we reject attachments at request build
//!   time with `MapperError::AttachmentUnsupported`.
//!
//! - **`Thinking` content**. OpenAI's public Chat Completions surface
//!   doesn't expose a reasoning channel. DeepSeek-style
//!   `reasoning_content` would need a provider-specific extension.

use super::client::{
    ChatChunk, ChatMessage, ChatRequest, ChunkToolCallDelta, StreamOptions, ToolCallFunction,
    ToolCallReplay, ToolDecl, ToolDeclFunction,
};
use crate::backend::TokenEventV2;
use inferd_proto::v2::{
    ContentBlock, MessageV2, ResolvedV2, RoleV2, StopReasonV2, ToolCallId, UsageV2,
};

/// Errors building the wire request from a `ResolvedV2`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MapperError {
    /// The request used an attachment-bearing content block; the
    /// OpenAI-compat adapter doesn't ingest images/audio/video in v0.2
    /// (see module docs).
    #[error("openai-compat adapter does not support {0} attachments in v0.2")]
    AttachmentUnsupported(&'static str),
    /// `ContentBlock::Unknown` reached the mapper. The proto-side
    /// `resolve()` should have caught this; if we see it here it means
    /// the request was constructed in code that bypassed validation.
    #[error("openai-compat adapter received an unknown content-block type")]
    UnknownContentBlock,
    /// Tool result content nested another tool result, image, etc.
    /// OpenAI's `role: "tool"` message takes a string body — anything
    /// non-text inside a `ToolResult` content array is unrepresentable.
    #[error("openai-compat tool_result content must be text only")]
    NonTextToolResult,
}

/// Translate a `ResolvedV2` envelope into the `ChatRequest` we POST.
///
/// `model` is the upstream model identifier (e.g. `"gpt-4o-mini"` or
/// `"meta-llama/Llama-3.1-8B-Instruct"`); inferd doesn't pick it — the
/// `OpenAiCompatConfig` does.
pub(super) fn request_from_resolved(
    resolved: &ResolvedV2,
    model: &str,
) -> Result<ChatRequest, MapperError> {
    if !resolved.attachments.is_empty() {
        // Belt-and-braces: capabilities() advertises vision/audio off,
        // so the daemon should never dispatch an attachment-bearing
        // request to us. Emit a structured error rather than a
        // confusing wire failure on the upstream's side.
        return Err(MapperError::AttachmentUnsupported("multimodal"));
    }

    let mut messages = Vec::with_capacity(resolved.messages.len());
    for msg in &resolved.messages {
        flatten_message(msg, &mut messages)?;
    }

    let tools = resolved
        .tools
        .iter()
        .map(|t| ToolDecl {
            kind: "function".to_string(),
            function: ToolDeclFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect();

    Ok(ChatRequest {
        model: model.to_string(),
        messages,
        stream: true,
        temperature: resolved.temperature,
        top_p: resolved.top_p,
        max_tokens: resolved.max_tokens,
        tools,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
    })
}

/// One v2 `MessageV2` may need to expand into multiple OpenAI messages
/// — a single user-role message containing `ToolResult` blocks
/// becomes a separate `role: "tool"` message per result, since the
/// OpenAI shape pairs results 1:1 with prior tool calls.
fn flatten_message(msg: &MessageV2, out: &mut Vec<ChatMessage>) -> Result<(), MapperError> {
    let role_str = role_to_str(msg.role);

    // Walk content blocks; collate text + tool_calls for the primary
    // message, and emit separate `role: "tool"` messages for any
    // ToolResult blocks (they don't share an envelope with text in
    // OpenAI's shape).
    let mut text_buf = String::new();
    let mut tool_calls: Vec<ToolCallReplay> = Vec::new();
    let mut tool_results: Vec<(ToolCallId, String)> = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                text_buf.push_str(text);
            }
            ContentBlock::ToolUse {
                tool_call_id,
                name,
                input,
            } => {
                tool_calls.push(ToolCallReplay {
                    id: tool_call_id.as_str().to_string(),
                    kind: "function".to_string(),
                    function: ToolCallFunction {
                        name: name.clone(),
                        arguments: serde_json::to_string(input)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                });
            }
            ContentBlock::ToolResult {
                tool_call_id,
                content,
            } => {
                let body = tool_result_to_string(content)?;
                tool_results.push((tool_call_id.clone(), body));
            }
            ContentBlock::Image { .. } => {
                return Err(MapperError::AttachmentUnsupported("image"));
            }
            ContentBlock::Audio { .. } => {
                return Err(MapperError::AttachmentUnsupported("audio"));
            }
            ContentBlock::Video { .. } => {
                return Err(MapperError::AttachmentUnsupported("video"));
            }
            ContentBlock::Unknown => {
                return Err(MapperError::UnknownContentBlock);
            }
        }
    }

    // Emit the primary message if it has any content. A user message
    // composed entirely of ToolResults skips this — the tool results
    // become the message stream on their own.
    let has_primary = !text_buf.is_empty() || !tool_calls.is_empty();
    if has_primary {
        out.push(ChatMessage {
            role: role_str.to_string(),
            content: if text_buf.is_empty() {
                None
            } else {
                Some(text_buf)
            },
            tool_calls,
            tool_call_id: None,
            name: None,
        });
    }

    // Emit one `role: "tool"` message per ToolResult, addressed by id.
    for (id, body) in tool_results {
        out.push(ChatMessage {
            role: "tool".to_string(),
            content: Some(body),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.as_str().to_string()),
            name: None,
        });
    }

    Ok(())
}

fn tool_result_to_string(content: &[ContentBlock]) -> Result<String, MapperError> {
    let mut out = String::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => out.push_str(text),
            _ => return Err(MapperError::NonTextToolResult),
        }
    }
    Ok(out)
}

fn role_to_str(role: RoleV2) -> &'static str {
    match role {
        RoleV2::System => "system",
        RoleV2::User => "user",
        RoleV2::Assistant => "assistant",
    }
}

// --- SSE chunk accumulator -----------------------------------------

/// Per-tool-call buffer for SSE deltas.
#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: Option<String>,
    name: String,
    arguments: String,
}

/// Accumulates SSE chunks and produces a stream of `TokenEventV2`.
///
/// OpenAI's wire emits text and tool-call deltas through the same
/// `delta` object on each chunk; tool-call deltas are scattered across
/// many chunks and only complete when `finish_reason` arrives. We
/// buffer per `index` so concurrent tool calls in one assistant turn
/// don't interleave.
#[derive(Debug, Default)]
pub(super) struct ChunkAccumulator {
    /// Buffered tool-call payloads keyed by their `index` slot. Sized
    /// dynamically — usually 1.
    tool_calls: Vec<ToolCallBuffer>,
    /// Captured `usage` from the trailing chunk (set when
    /// `stream_options.include_usage` is on).
    usage: Option<UsageV2>,
    /// Final `finish_reason` — drives the v2 `StopReasonV2`.
    finish_reason: Option<String>,
}

impl ChunkAccumulator {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Process one SSE chunk. Returns the events to emit (zero or
    /// more `Text` deltas).
    pub(super) fn ingest(&mut self, chunk: ChatChunk) -> Vec<TokenEventV2> {
        let mut out = Vec::new();

        if let Some(u) = chunk.usage {
            self.usage = Some(UsageV2 {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
            });
        }

        for choice in chunk.choices {
            if let Some(text) = choice.delta.content
                && !text.is_empty()
            {
                out.push(TokenEventV2::Text(text));
            }
            for tc in choice.delta.tool_calls {
                self.absorb_tool_call_delta(tc);
            }
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(reason);
            }
        }

        out
    }

    fn absorb_tool_call_delta(&mut self, delta: ChunkToolCallDelta) {
        // Grow the buffer to fit the index slot.
        while self.tool_calls.len() <= delta.index {
            self.tool_calls.push(ToolCallBuffer::default());
        }
        let slot = &mut self.tool_calls[delta.index];
        if let Some(id) = delta.id {
            slot.id = Some(id);
        }
        if let Some(func) = delta.function {
            if let Some(name) = func.name {
                slot.name.push_str(&name);
            }
            if let Some(args) = func.arguments {
                slot.arguments.push_str(&args);
            }
        }
    }

    /// Drain any buffered tool calls. Called when the SSE stream
    /// terminates so the accumulator can hand back the final
    /// `ToolUse` events plus the `Done` frame.
    pub(super) fn finalize(mut self) -> Vec<TokenEventV2> {
        let mut out = Vec::new();

        // Order: any pending ToolUse blocks first, then Done.
        let stop_reason_hint = self.finish_reason.clone();
        let calls = std::mem::take(&mut self.tool_calls);
        for buf in calls {
            let Some(id) = buf.id else {
                // No id ever arrived. Skip — emitting a malformed
                // ToolUse is worse than dropping it; the consumer's
                // round-trip would fail to address the result back.
                continue;
            };
            // Empty arguments → `{}` so the consumer always gets valid
            // JSON; OpenAI sometimes streams nothing for zero-arg tools.
            let args = if buf.arguments.is_empty() {
                "{}".to_string()
            } else {
                buf.arguments
            };
            let input: serde_json::Value =
                serde_json::from_str(&args).unwrap_or(serde_json::Value::Null);
            out.push(TokenEventV2::ToolUse {
                tool_call_id: ToolCallId(id),
                name: buf.name,
                input,
            });
        }

        let usage = self.usage.unwrap_or(UsageV2 {
            input_tokens: 0,
            output_tokens: 0,
        });
        let stop_reason = match stop_reason_hint.as_deref() {
            Some("tool_calls") | Some("function_call") => StopReasonV2::ToolUse,
            Some("length") => StopReasonV2::MaxTokens,
            Some("stop") => StopReasonV2::EndTurn,
            // No finish_reason on any chunk usually means the upstream
            // closed the stream uncleanly. Surface as Error so the
            // daemon translates to BackendUnavailable.
            None => StopReasonV2::Error,
            // content_filter and any unrecognised reason: treat as a
            // clean end-of-turn — the model stopped on its own
            // criteria, and we have no v2 variant for filtering.
            Some(_) => StopReasonV2::EndTurn,
        };

        out.push(TokenEventV2::Done { stop_reason, usage });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai_compat::client::{
        ChunkChoice, ChunkDelta, ChunkToolCallDelta, ChunkToolCallFunctionDelta, ChunkUsage,
    };
    use inferd_proto::v2::{ContentBlock, MessageV2, RequestV2, RoleV2, Tool};
    use serde_json::json;

    fn resolved_with_messages(messages: Vec<MessageV2>) -> ResolvedV2 {
        RequestV2 {
            id: "req-1".into(),
            messages,
            ..Default::default()
        }
        .resolve()
        .unwrap()
    }

    #[test]
    fn text_only_request_round_trips() {
        let r = resolved_with_messages(vec![
            MessageV2 {
                role: RoleV2::System,
                content: vec![ContentBlock::Text {
                    text: "be terse".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            },
        ]);
        let req = request_from_resolved(&r, "test-model").unwrap();
        assert_eq!(req.model, "test-model");
        assert!(req.stream);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[0].content.as_deref(), Some("be terse"));
        assert_eq!(req.messages[1].role, "user");
        assert_eq!(req.messages[1].content.as_deref(), Some("hello"));
        assert!(req.tools.is_empty());
    }

    #[test]
    fn tools_translate_to_function_decls() {
        let mut r = resolved_with_messages(vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "go".into() }],
        }]);
        r.tools = vec![Tool {
            name: "lookup".into(),
            description: "look something up".into(),
            input_schema: json!({"type": "object"}),
        }];
        let req = request_from_resolved(&r, "m").unwrap();
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].kind, "function");
        assert_eq!(req.tools[0].function.name, "lookup");
    }

    #[test]
    fn assistant_tool_use_replays_as_tool_calls() {
        let r = resolved_with_messages(vec![
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text { text: "go".into() }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![ContentBlock::ToolUse {
                    tool_call_id: ToolCallId("call_1".into()),
                    name: "lookup".into(),
                    input: json!({"q": "x"}),
                }],
            },
        ]);
        let req = request_from_resolved(&r, "m").unwrap();
        assert_eq!(req.messages.len(), 2);
        let asst = &req.messages[1];
        assert_eq!(asst.role, "assistant");
        assert!(asst.content.is_none());
        assert_eq!(asst.tool_calls.len(), 1);
        assert_eq!(asst.tool_calls[0].id, "call_1");
        assert_eq!(asst.tool_calls[0].function.name, "lookup");
        assert_eq!(asst.tool_calls[0].function.arguments, r#"{"q":"x"}"#);
    }

    #[test]
    fn tool_result_emits_tool_role_message() {
        let r = resolved_with_messages(vec![
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text { text: "go".into() }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![ContentBlock::ToolUse {
                    tool_call_id: ToolCallId("call_1".into()),
                    name: "lookup".into(),
                    input: json!({}),
                }],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::ToolResult {
                    tool_call_id: ToolCallId("call_1".into()),
                    content: vec![ContentBlock::Text {
                        text: "the answer is 42".into(),
                    }],
                }],
            },
        ]);
        let req = request_from_resolved(&r, "m").unwrap();
        // Three input messages → user / assistant / tool. The user
        // turn that contained only the ToolResult has *no* primary
        // message in the OpenAI shape — it becomes a `role: "tool"`
        // message addressed by tool_call_id.
        assert_eq!(req.messages.len(), 3);
        let tool_msg = &req.messages[2];
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(tool_msg.content.as_deref(), Some("the answer is 42"));
    }

    #[test]
    fn image_attachment_block_is_rejected() {
        // Build manually because RequestV2::resolve would reject an
        // unresolved attachment_id; we want to test the mapper guard,
        // not the proto guard, so we construct ResolvedV2 directly.
        let r = ResolvedV2 {
            wire_version: inferd_proto::v2::WIRE_VERSION,
            id: "x".into(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Image {
                    attachment_id: "img-1".into(),
                }],
            }],
            attachments: Vec::new(),
            tools: Vec::new(),
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            stream: None,
            response_format: None,
        };
        let err = request_from_resolved(&r, "m").unwrap_err();
        assert_eq!(err, MapperError::AttachmentUnsupported("image"));
    }

    fn chunk_with_text(text: &str) -> ChatChunk {
        ChatChunk {
            choices: vec![ChunkChoice {
                delta: ChunkDelta {
                    content: Some(text.to_string()),
                    tool_calls: Vec::new(),
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    #[test]
    fn accumulator_passes_text_through_and_emits_done() {
        let mut acc = ChunkAccumulator::new();
        let evs = acc.ingest(chunk_with_text("hello"));
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], TokenEventV2::Text(ref s) if s == "hello"));
        let evs = acc.ingest(chunk_with_text(" world"));
        assert_eq!(evs.len(), 1);
        // Final chunk: finish_reason + usage.
        let last = ChatChunk {
            choices: vec![ChunkChoice {
                delta: ChunkDelta::default(),
                finish_reason: Some("stop".into()),
            }],
            usage: Some(ChunkUsage {
                prompt_tokens: 7,
                completion_tokens: 3,
            }),
        };
        let evs = acc.ingest(last);
        assert!(evs.is_empty());
        let final_evs = acc.finalize();
        assert_eq!(final_evs.len(), 1);
        match &final_evs[0] {
            TokenEventV2::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReasonV2::EndTurn);
                assert_eq!(usage.input_tokens, 7);
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn accumulator_assembles_tool_call_across_deltas() {
        let mut acc = ChunkAccumulator::new();

        // Chunk 1: id + name.
        acc.ingest(ChatChunk {
            choices: vec![ChunkChoice {
                delta: ChunkDelta {
                    content: None,
                    tool_calls: vec![ChunkToolCallDelta {
                        index: 0,
                        id: Some("call_42".into()),
                        function: Some(ChunkToolCallFunctionDelta {
                            name: Some("lookup".into()),
                            arguments: None,
                        }),
                    }],
                },
                finish_reason: None,
            }],
            usage: None,
        });
        // Chunk 2: arguments first half.
        acc.ingest(ChatChunk {
            choices: vec![ChunkChoice {
                delta: ChunkDelta {
                    content: None,
                    tool_calls: vec![ChunkToolCallDelta {
                        index: 0,
                        id: None,
                        function: Some(ChunkToolCallFunctionDelta {
                            name: None,
                            arguments: Some(r#"{"q":"x"#.into()),
                        }),
                    }],
                },
                finish_reason: None,
            }],
            usage: None,
        });
        // Chunk 3: arguments second half.
        acc.ingest(ChatChunk {
            choices: vec![ChunkChoice {
                delta: ChunkDelta {
                    content: None,
                    tool_calls: vec![ChunkToolCallDelta {
                        index: 0,
                        id: None,
                        function: Some(ChunkToolCallFunctionDelta {
                            name: None,
                            arguments: Some(r#"y"}"#.into()),
                        }),
                    }],
                },
                finish_reason: None,
            }],
            usage: None,
        });
        // Chunk 4: terminal.
        acc.ingest(ChatChunk {
            choices: vec![ChunkChoice {
                delta: ChunkDelta::default(),
                finish_reason: Some("tool_calls".into()),
            }],
            usage: Some(ChunkUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            }),
        });

        let evs = acc.finalize();
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            TokenEventV2::ToolUse {
                tool_call_id,
                name,
                input,
            } => {
                assert_eq!(tool_call_id.as_str(), "call_42");
                assert_eq!(name, "lookup");
                assert_eq!(input, &json!({"q": "xy"}));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match &evs[1] {
            TokenEventV2::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReasonV2::ToolUse);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn accumulator_treats_missing_finish_reason_as_error() {
        let mut acc = ChunkAccumulator::new();
        acc.ingest(chunk_with_text("hi"));
        // No terminal chunk arrives.
        let evs = acc.finalize();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            TokenEventV2::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReasonV2::Error);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }
}
