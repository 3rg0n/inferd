//! Gemma 4 prompt-format renderer.
//!
//! Translates a `ResolvedV2` (validated typed-content-block request)
//! into the byte-exact prompt string Gemma 4 expects, plus an ordered
//! `Vec<&Attachment>` so the engine adapter can hand the same
//! sequence to `mtmd_tokenize`.
//!
//! Reference: `docs/text.function.calling.with.gemma.4.md` and
//! `docs/thinking.mode.in.gemma.md`. The control-token vocabulary is
//! frozen by the upstream Gemma 4 chat template; this module
//! mirrors it, it does not invent it.
//!
//! ## Format (canonical, from upstream docs)
//!
//! Whole-prompt envelope:
//! ```text
//! <bos><|turn>system
//! {system_text}{tool_declarations}<turn|>
//! <|turn>user
//! {user_content}<turn|>
//! <|turn>model
//! {assistant_content}<turn|>
//! ...
//! <|turn>model      <-- generation prompt (added by add_generation_prompt=true)
//! ```
//!
//! Tool declarations live inside the system turn:
//! ```text
//! <|tool>declaration:NAME{description:<|"|>...<|"|>,parameters:{...}}<tool|>
//! ```
//!
//! Tool call (assistant emits these mid-stream):
//! ```text
//! <|tool_call>call:NAME{KEY:<|"|>VALUE<|"|>,...}<tool_call|>
//! ```
//!
//! Tool response (consumer's follow-up; appended after the model's
//! tool_call within the same model turn):
//! ```text
//! <|tool_response>response:NAME{KEY:VALUE,...}<tool_response|>
//! ```
//!
//! Image / audio attachments inside a content array become the
//! mtmd-default media marker `<__media__>` in the rendered text. The
//! engine adapter (Phase 3A) calls `mtmd_tokenize` with the same
//! prompt + the matching ordered bitmaps; mtmd splits the prompt at
//! the markers and splices the per-modality fence tokens
//! (`<start_of_image>...<end_of_image>`, etc.) in.

use inferd_proto::v2::{
    Attachment, AttachmentKind, ContentBlock, MessageV2, ResolvedV2, RoleV2, Tool,
};
use serde_json::Value;

/// The mtmd default media marker. The engine adapter sees this
/// substring in the rendered prompt and replaces it (via
/// `mtmd_tokenize`) with the per-modality fence tokens for the
/// associated bitmap.
pub const MEDIA_MARKER: &str = "<__media__>";

/// Output of [`Gemma4Renderer::render`].
///
/// `prompt` is the flat string ready for `mtmd_tokenize`.
/// `attachments` lists the attachments referenced by media markers
/// in `prompt`, in the order the markers appear. The engine adapter
/// supplies them to `mtmd_tokenize` in this same order so each
/// marker resolves to the correct bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gemma4Rendered<'a> {
    /// Flat prompt with control tokens + media markers.
    pub prompt: String,
    /// Attachments in the order their content blocks appear.
    pub attachments: Vec<&'a Attachment>,
}

/// Errors from [`Gemma4Renderer::render`].
#[derive(Debug, thiserror::Error)]
pub enum Gemma4RenderError {
    /// A content block referenced an attachment_id that doesn't
    /// resolve to a `ResolvedV2::attachments[]` entry. (This should
    /// have been caught by `RequestV2::resolve()`; arriving here
    /// means the resolved input was constructed bypassing
    /// validation.)
    #[error(
        "messages[{message_index}].content[{block_index}]: attachment {attachment_id:?} not found"
    )]
    DanglingAttachment {
        /// Which message in `messages[]`.
        message_index: usize,
        /// Which content block in that message.
        block_index: usize,
        /// The id that didn't resolve.
        attachment_id: String,
    },
    /// A content block carried `ContentBlock::Unknown`. The daemon
    /// rejects this earlier in `RequestV2::resolve`; if it gets here
    /// we treat it as an internal invariant violation.
    #[error("messages[{message_index}].content[{block_index}] is an unknown content-block type")]
    UnknownBlock {
        /// Which message in `messages[]`.
        message_index: usize,
        /// Which content block in that message.
        block_index: usize,
    },
}

/// Stateless Gemma 4 renderer. Construct with [`Gemma4Renderer::new`]
/// and call [`render`](Self::render) per request.
#[derive(Debug, Default)]
pub struct Gemma4Renderer;

impl Gemma4Renderer {
    /// Construct a renderer.
    pub fn new() -> Self {
        Self
    }

    /// Render `resolved` into a flat Gemma 4 prompt + an ordered
    /// list of referenced attachments.
    pub fn render<'a>(
        &self,
        resolved: &'a ResolvedV2,
    ) -> Result<Gemma4Rendered<'a>, Gemma4RenderError> {
        let mut prompt = String::with_capacity(512);
        let mut attachments: Vec<&Attachment> = Vec::new();

        // Lookup table for attachment_id -> Attachment. Built once;
        // resolve() guarantees uniqueness.
        let by_id: std::collections::HashMap<&str, &Attachment> = resolved
            .attachments
            .iter()
            .map(|a| (a.id.as_str(), a))
            .collect();

        // <bos> opens the prompt. Gemma's tokenizer maps this to the
        // BOS token at tokenize time; we emit the literal string.
        prompt.push_str("<bos>");

        for (mi, msg) in resolved.messages.iter().enumerate() {
            // First system message is the natural place to embed
            // tool declarations — matches the reference output
            // verbatim. If the request has tools[] but no system
            // message, we synthesise an empty-content system turn
            // *before* the first message so the tool block has a
            // home; this is what the upstream chat template does
            // (see line 121 of `text.function.calling.with.gemma.4.md`
            // — tool declarations appear inside an empty system turn
            // when the user didn't supply one).
            if mi == 0 && !resolved.tools.is_empty() && msg.role != RoleV2::System {
                prompt.push_str("<|turn>system\n");
                render_tool_declarations(&mut prompt, &resolved.tools);
                prompt.push_str("<turn|>\n");
            }

            render_message(
                &mut prompt,
                mi,
                msg,
                &by_id,
                &mut attachments,
                &resolved.tools,
            )?;
        }

        // Generation prompt: the trailing `<|turn>model\n` with no
        // closing `<turn|>` signals the model to start emitting its
        // turn (matches `add_generation_prompt=true` in the upstream
        // example).
        prompt.push_str("<|turn>model\n");

        Ok(Gemma4Rendered {
            prompt,
            attachments,
        })
    }
}

fn render_message<'a>(
    out: &mut String,
    mi: usize,
    msg: &'a MessageV2,
    by_id: &std::collections::HashMap<&str, &'a Attachment>,
    attachments: &mut Vec<&'a Attachment>,
    tools: &[Tool],
) -> Result<(), Gemma4RenderError> {
    out.push_str(role_open_tag(msg.role));
    out.push('\n');

    // System turn embeds tool declarations after any content.
    let is_system = msg.role == RoleV2::System;

    for (bi, block) in msg.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text } => {
                out.push_str(text);
            }
            ContentBlock::Image { attachment_id }
            | ContentBlock::Audio { attachment_id }
            | ContentBlock::Video { attachment_id } => {
                let att = by_id.get(attachment_id.as_str()).ok_or_else(|| {
                    Gemma4RenderError::DanglingAttachment {
                        message_index: mi,
                        block_index: bi,
                        attachment_id: attachment_id.clone(),
                    }
                })?;
                out.push_str(MEDIA_MARKER);
                attachments.push(*att);
                // Mtmd validates kind matches content-block variant
                // when it tokenises; we don't need to. (Belt: if
                // they ever disagree, mtmd_tokenize errors loudly,
                // and the lifecycle layer translates that to
                // ErrorCodeV2::AttachmentUnsupported.)
                let _ = AttachmentKind::Image; // keep the use in scope
            }
            ContentBlock::ToolUse {
                tool_call_id: _,
                name,
                input,
            } => {
                // Assistant turns can replay prior tool calls for
                // context. The id we generated when the model first
                // emitted the call is dropped here — Gemma's wire
                // format doesn't carry an id back into the prompt;
                // it pairs by position. (Our id is for the
                // consumer-facing v2 wire, where positional pairing
                // would be fragile across pipelining.)
                out.push_str("<|tool_call>call:");
                out.push_str(name);
                out.push('{');
                render_args_inline(out, input);
                out.push_str("}<tool_call|>");
            }
            ContentBlock::ToolResult {
                tool_call_id: _,
                content,
            } => {
                // Per the upstream docs the tool response is rendered
                // inside the same model turn as the tool_call —
                // i.e. the response continues the assistant's turn,
                // it's not a separate turn. The consumer constructs
                // a follow-up RequestV2 with the ToolResult inside
                // a `User`-role message (matches Anthropic), but
                // Gemma's flat-prompt format wraps it into the
                // model turn. We honor the upstream convention: emit
                // the response *inline* inside whatever turn this
                // ToolResult sits in.
                out.push_str("<|tool_response>");
                if let Some(tool_name) = guess_tool_name(content, tools) {
                    out.push_str("response:");
                    out.push_str(tool_name);
                    out.push('{');
                    render_text_only_response(out, content);
                    out.push('}');
                } else {
                    // Couldn't infer a tool name — emit raw content.
                    // Gemma will treat this as freeform tool output;
                    // worse than a perfect render but doesn't crash.
                    render_text_only_response(out, content);
                }
                out.push_str("<tool_response|>");
            }
            ContentBlock::Unknown => {
                return Err(Gemma4RenderError::UnknownBlock {
                    message_index: mi,
                    block_index: bi,
                });
            }
        }
    }

    if is_system && !tools.is_empty() {
        render_tool_declarations(out, tools);
    }

    out.push_str("<turn|>\n");
    Ok(())
}

fn role_open_tag(role: RoleV2) -> &'static str {
    match role {
        RoleV2::System => "<|turn>system",
        RoleV2::User => "<|turn>user",
        // v2 calls assistant turns "assistant"; Gemma's wire token
        // is "model". The renderer translates.
        RoleV2::Assistant => "<|turn>model",
    }
}

fn render_tool_declarations(out: &mut String, tools: &[Tool]) {
    for tool in tools {
        out.push_str("<|tool>declaration:");
        out.push_str(&tool.name);
        out.push('{');
        out.push_str("description:<|\"|>");
        out.push_str(&tool.description);
        out.push_str("<|\"|>,parameters:");
        render_schema(out, &tool.input_schema);
        out.push('}');
        out.push_str("<tool|>");
    }
}

/// Render a JSON Schema value into Gemma's wire format. The format
/// is JSON-shaped but with `<|"|>` instead of `"` around strings.
/// Gemma's tokenizer treats `<|"|>` as a special token, which
/// distinguishes string content from structural punctuation in the
/// rendered prompt.
fn render_schema(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str("<|\"|>");
            out.push_str(s);
            out.push_str("<|\"|>");
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                render_schema(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut first = true;
            for (k, v) in map {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(k);
                out.push(':');
                render_schema(out, v);
            }
            out.push('}');
        }
    }
}

/// Render tool-call arguments inline. Gemma's format uses bare keys
/// plus `<|"|>`-quoted string values (same as schema rendering).
fn render_args_inline(out: &mut String, value: &Value) {
    if let Value::Object(map) = value {
        let mut first = true;
        for (k, v) in map {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(k);
            out.push(':');
            render_schema(out, v);
        }
    } else {
        // Defensive: a non-object input shouldn't happen for
        // tool_use blocks (the model emits objects). Render whatever
        // it is so we don't lose data.
        render_schema(out, value);
    }
}

/// Try to infer the tool name from ToolResult context. The v2 wire
/// shape names the tool via tool_call_id; Gemma's wire shape needs
/// the literal name. If the request's tools[] has exactly one entry,
/// we use it; otherwise we return None and the caller emits raw
/// content.
fn guess_tool_name<'a>(_content: &[ContentBlock], tools: &'a [Tool]) -> Option<&'a str> {
    if tools.len() == 1 {
        Some(tools[0].name.as_str())
    } else {
        None
    }
}

/// Render a tool-result content array as a flat key:value object.
/// Today we only handle text-only ToolResult content (the typical
/// case — middleware passes the tool's stringified output back in
/// as a single Text block). If the consumer nests further structure
/// (a nested image, etc.), we render only the top-level text and
/// drop the rest. Phase 4B will revisit this if real consumers need
/// richer tool_result content.
fn render_text_only_response(out: &mut String, content: &[ContentBlock]) {
    for block in content {
        if let ContentBlock::Text { text } = block {
            // Try to parse as JSON; if it parses to an object, emit
            // it as structured wire format. Otherwise (parse failure
            // or non-object value), emit the raw text.
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) {
                let mut first = true;
                for (k, v) in map {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    out.push_str(&k);
                    out.push(':');
                    render_schema(out, &v);
                }
            } else {
                out.push_str(text);
            }
        }
    }
}
