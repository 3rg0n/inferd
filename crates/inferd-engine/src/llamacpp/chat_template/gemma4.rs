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

use super::tool_grammar::{ToolGrammar, escape_literal, push_exclusion_rules, regex_escape};
use super::{ChatFamily, ChatRenderer, MEDIA_MARKER, RenderError, Rendered};
use inferd_proto::v2::{
    Attachment, ContentBlock, MessageV2, ResolvedV2, RoleV2, Tool, ToolCallId, ToolChoice,
};
use serde_json::Value;
use std::fmt::Write as _;

/// The tool-call opener. Also the lazy-grammar trigger: the model
/// emitting this substring is the moment it commits to a call.
const TOOL_OPEN: &str = "<|tool_call>";
/// The tool-call closer.
const TOOL_CLOSE: &str = "<tool_call|>";
/// Gemma's string fence, standing in for JSON's `"`.
const STRING_FENCE: &str = "<|\"|>";

/// Stateless Gemma 4 renderer. Construct with [`Gemma4Renderer::new`]
/// and call [`ChatRenderer::render`] per request.
#[derive(Debug, Default)]
pub struct Gemma4Renderer;

impl Gemma4Renderer {
    /// Construct a renderer.
    pub fn new() -> Self {
        Self
    }
}

impl ChatRenderer for Gemma4Renderer {
    fn family(&self) -> ChatFamily {
        ChatFamily::Gemma4
    }

    fn render<'a>(&self, resolved: &'a ResolvedV2) -> Result<Rendered<'a>, RenderError> {
        let mut prompt = String::with_capacity(512);
        let mut attachments: Vec<&Attachment> = Vec::new();

        // Lookup table for attachment_id -> Attachment. Built once;
        // resolve() guarantees uniqueness.
        let by_id: std::collections::HashMap<&str, &Attachment> =
            resolved.attachments.iter().map(|a| (a.id(), a)).collect();

        // Lookup table for tool_call_id -> tool name. Walk all messages
        // and harvest every ToolUse so a later ToolResult can pair via
        // tool_call_id (per ADR 0015 §"v2 ContentBlock variants"). The
        // last write wins on duplicates, but ResolvedV2 doesn't enforce
        // tool_call_id uniqueness — duplicates are pathological caller
        // error and the second one effectively shadows the first.
        let tool_name_by_call_id: std::collections::HashMap<&ToolCallId, &str> = resolved
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse {
                    tool_call_id, name, ..
                } => Some((tool_call_id, name.as_str())),
                _ => None,
            })
            .collect();

        // <bos> opens the prompt. Gemma's tokenizer maps this to the
        // BOS token at tokenize time; we emit the literal string.
        prompt.push_str("<bos>");

        // Thinking ("reasoning") activation (ADR 0013): when the request
        // asks for it, Gemma 4 is turned on by placing the `<|think|>`
        // token in the system turn, consolidated with any other system
        // instructions / tool declarations (per the GA prompt-format spec
        // + the released GGUF chat_template). The model then emits its
        // reasoning as `<|channel>thought…<channel|>`, which the parser
        // separates onto `thinking` response blocks.
        let thinking = resolved.thinking.unwrap_or(false);

        // Does the conversation already open with a system message? If so
        // `<|think|>` is injected into it (render_message). If not — but
        // thinking and/or tools need a system turn — we synthesise one
        // before the first message.
        let has_leading_system = resolved
            .messages
            .first()
            .is_some_and(|m| m.role == RoleV2::System);
        let needs_synth_system = !has_leading_system && (thinking || !resolved.tools.is_empty());

        for (mi, msg) in resolved.messages.iter().enumerate() {
            // Synthesise a system turn before the first message when the
            // caller supplied none but we need one for `<|think|>` and/or
            // tool declarations. This mirrors the upstream chat template
            // (tool declarations live in an empty system turn when the
            // user didn't supply one); `<|think|>` joins it at the front.
            if mi == 0 && needs_synth_system {
                prompt.push_str("<|turn>system\n");
                if thinking {
                    prompt.push_str("<|think|>");
                }
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
                &tool_name_by_call_id,
                // Inject `<|think|>` into the FIRST message only when it's
                // the system turn (otherwise it was handled by the
                // synthesised turn above).
                thinking && mi == 0 && msg.role == RoleV2::System,
            )?;
        }

        // Generation prompt: the trailing `<|turn>model\n` with no
        // closing `<turn|>` signals the model to start emitting its
        // turn (matches `add_generation_prompt=true` in the upstream
        // example).
        prompt.push_str("<|turn>model\n");

        Ok(Rendered {
            prompt,
            attachments,
        })
    }

    fn tool_call_grammar(
        &self,
        choice: ToolChoice,
        tools: &[Tool],
    ) -> Result<Option<ToolGrammar>, RenderError> {
        match choice {
            // `auto` is the pre-`tool_choice` behaviour plus a safety
            // net. A *lazy* grammar constrains nothing until the model
            // emits `<|tool_call>`; from that point the syntax is
            // pinned, so the model can no longer produce a call body
            // the parser will reject as `Malformed` — which today
            // aborts an otherwise good generation. It cannot force a
            // call, and must not: that is what `required` is for.
            ToolChoice::Auto => Ok(Some(ToolGrammar {
                gbnf: gemma4_tool_call_gbnf(tools, false),
                triggers: vec![regex_escape(TOOL_OPEN)],
            })),

            // `required` is the mode that needs real enforcement. An
            // eager grammar whose root demands one complete call means
            // no stack is empty until the closer is emitted, and
            // `llama_grammar_apply_impl` masks every end-of-generation
            // token while that holds — so the model cannot end its
            // turn with prose instead.
            ToolChoice::Required => Ok(Some(ToolGrammar {
                gbnf: gemma4_tool_call_gbnf(tools, true),
                triggers: Vec::new(),
            })),

            // `none` means the tools are visible but off-limits this
            // turn. Upstream builds no grammar here, which leaves the
            // mode advisory. We constrain instead, because a model
            // that calls a forbidden tool anyway produces a
            // `ToolUse` block the caller explicitly said it would not
            // handle.
            //
            // The constraint has to be over *text*, not token ids: a
            // `!<|tool_call>` token rule would only bar the single
            // control token, while `<`, `|tool`, `_call>` spells the
            // same opener in ordinary pieces and inferd's parser
            // scans the detokenised text, so it would fire on the
            // spelled-out form. The exclusion automaton bars the
            // string however it is tokenised.
            ToolChoice::None => Ok(Some(ToolGrammar {
                gbnf: gemma4_no_tool_call_gbnf(),
                triggers: Vec::new(),
            })),

            // `resolve()` rejects `Unknown` before it can reach a
            // renderer. Treating it as `auto` here would resurrect the
            // fail-open the wire-level check exists to close, so it
            // refuses.
            ToolChoice::Unknown => Err(RenderError::Unsupported {
                family: ChatFamily::Gemma4,
                feature: "an unrecognised tool_choice mode",
            }),
        }
    }
}

/// The value-shape rules shared by every mode that admits a call
/// body, transposed from `common_chat_params_init_gemma4`
/// (`vendor/llama.cpp/common/chat.cpp`).
///
/// Two deliberate narrowings versus upstream, both because inferd
/// parses these bodies with [`super::super::tool_parser`] rather than
/// upstream's PEG, and a grammar wider than its own parser admits
/// output the parser then rejects as `Malformed`:
///
/// 1. **Keys.** Upstream's `gemma4-dict-key-name` is `[^:}]+`, which
///    permits `{a b:1}` and `{"a":1}`. inferd's `quote_bare_keys`
///    quotes only `[A-Za-z][A-Za-z0-9_]*`, so anything else reaches
///    `serde_json` unquoted and fails to parse. The rule is narrowed
///    to exactly that identifier shape.
/// 2. **String content.** inferd converts `<|"|>` fences to `"` by
///    substitution *before* parsing, so a literal `"` or `\` inside
///    the content becomes a stray quote or a bogus escape in the JSON
///    it hands to `serde_json`. Both are excluded, along with raw
///    control characters, which JSON forbids unescaped.
///
/// Neither narrowing loses expressible arguments: a string value that
/// needs a quote is not currently representable on this wire in
/// either direction, so the grammar refusing it converts a
/// mid-stream parse failure into a token the model never emits.
fn push_value_rules(out: &mut String) {
    // `"` and `\` break the fence→quote substitution described above;
    // \x00-\x1f are the control characters JSON forbids bare.
    //
    // Excluding `"` also removes the need for an exclusion automaton
    // over the closing fence: `<|"|>` contains a `"`, so no string
    // this rule admits can contain the fence, and the content run
    // terminates unambiguously at it.
    out.push_str("str-safe ::= [^\\\"\\\\\\x00-\\x1f]\n");
    let fence = escape_literal(STRING_FENCE);
    let _ = writeln!(out, "gemma4-string ::= \"{fence}\" str-content \"{fence}\"");
    out.push_str("str-content ::= str-safe*\n");
    out.push_str("gemma4-bool ::= \"true\" | \"false\"\n");
    out.push_str("gemma4-null ::= \"null\"\n");
    out.push_str(
        "gemma4-number ::= \"-\"? (\"0\" | [1-9] [0-9]*) (\".\" [0-9]+)? ([eE] [-+]? [0-9]+)?\n",
    );
    // Narrowed to what `quote_bare_keys` will quote — see the doc
    // comment. Upstream: `[^:}]+`.
    out.push_str("gemma4-dict-key ::= [a-zA-Z] [a-zA-Z0-9_]* \":\"\n");
    out.push_str("gemma4-dict-kv ::= gemma4-dict-key ws gemma4-value\n");
    out.push_str(
        "gemma4-dict ::= \"{\" ws (\"}\" | gemma4-dict-kv (\",\" ws gemma4-dict-kv)* ws \"}\")\n",
    );
    out.push_str(
        "gemma4-array ::= \"[\" ws (\"]\" | gemma4-value (\",\" ws gemma4-value)* ws \"]\")\n",
    );
    out.push_str(
        "gemma4-value ::= gemma4-string | gemma4-dict | gemma4-array | gemma4-number | gemma4-bool | gemma4-null\n",
    );
    // Only spaces and tabs: `quote_bare_keys` skips exactly those when
    // looking back for a key boundary, so a newline before a key would
    // leave it unquoted and unparseable.
    out.push_str("ws ::= [ \\t]*\n");
}

/// GBNF constraining a Gemma 4 tool call. `required` forces exactly one
/// call; otherwise the root matches one call and is used lazily.
///
/// Only the tool *name* is constrained, not its arguments — a call to a
/// tool that was never declared is unusable to the caller, so masking
/// those names is the constraint that pays. Per-tool argument schemas
/// are left unconstrained, matching upstream, which carries a live TODO
/// there (`need to extend json-schema-to-grammar to produce more than
/// JSON rules`): Gemma's body is not JSON, so the shipped
/// `json_schema_to_gbnf` cannot express it and a second schema
/// compiler would have to be written to close the gap. Arguments are
/// still validated after parsing, they are simply not masked during
/// decoding.
fn gemma4_tool_call_gbnf(tools: &[Tool], required: bool) -> String {
    let mut g = String::with_capacity(1024);

    if required {
        // Eager. The root demands a complete call, so no stack is
        // empty until the closer lands and every EOG token stays
        // masked until then — that masking *is* the guarantee.
        //
        // The leading `prefix` allows content before the call: Gemma 4
        // reasons in a `<|channel>thought…<channel|>` block, and a
        // root of bare `tool-call` would mask the `<` that opens it,
        // forcing the model to call blind. `prefix` is the exclusion
        // automaton for the opener, so that content cannot itself
        // contain a call — one call, with room to think first.
        //
        // Every `prefix` state is nullable, so this does not weaken
        // the guarantee: nullable means the model *may* stop adding
        // content, never that it may stop before the call, because the
        // `tool-call` that follows is not optional.
        g.push_str("root ::= prefix-0 tool-call\n");
        push_exclusion_rules(&mut g, "prefix-", TOOL_OPEN);
    } else {
        // Lazy: `llama_grammar_accept_impl` replays output from the
        // trigger match onward, so the root starts at the opener and
        // needs no preceding-content rule — anything before the
        // trigger was never constrained at all.
        g.push_str("root ::= tool-call\n");
    }

    let open = escape_literal(TOOL_OPEN);
    let close = escape_literal(TOOL_CLOSE);
    let _ = writeln!(g, "tool-call ::= \"{open}call:\" tool-body \"{close}\"");

    // One alternative per declared tool. A tools array is non-empty
    // whenever a grammar is built (`resolve()` guarantees it), so this
    // alternation always has at least one branch — an empty one would
    // make the root unmatchable and, under `required`, mask every
    // token including EOG.
    g.push_str("tool-body ::=");
    for (i, tool) in tools.iter().enumerate() {
        if i > 0 {
            g.push_str(" |");
        }
        let _ = write!(g, " \"{}\" gemma4-dict", escape_literal(&tool.name));
    }
    g.push('\n');

    push_value_rules(&mut g);
    g
}

/// GBNF for `tool_choice: "none"` — any output that never contains the
/// tool-call opener.
///
/// Eager and unconditional: there is nothing to trigger on, since the
/// point is that the trigger must never occur.
fn gemma4_no_tool_call_gbnf() -> String {
    let mut g = String::with_capacity(512);
    g.push_str("root ::= no-tool-0\n");
    push_exclusion_rules(&mut g, "no-tool-", TOOL_OPEN);
    g
}

#[allow(clippy::too_many_arguments)]
fn render_message<'a>(
    out: &mut String,
    mi: usize,
    msg: &'a MessageV2,
    by_id: &std::collections::HashMap<&str, &'a Attachment>,
    attachments: &mut Vec<&'a Attachment>,
    tools: &[Tool],
    tool_name_by_call_id: &std::collections::HashMap<&'a ToolCallId, &'a str>,
    inject_think: bool,
) -> Result<(), RenderError> {
    out.push_str(role_open_tag(msg.role));
    out.push('\n');

    // System turn embeds tool declarations after any content.
    let is_system = msg.role == RoleV2::System;

    // Thinking activation: `<|think|>` leads the system turn, before any
    // system text or tool declarations (matches the GA chat_template).
    if inject_think && is_system {
        out.push_str("<|think|>");
    }

    for (bi, block) in msg.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text } => {
                out.push_str(text);
            }
            ContentBlock::Image { attachment_id }
            | ContentBlock::Audio { attachment_id }
            | ContentBlock::Video { attachment_id } => {
                let att = by_id.get(attachment_id.as_str()).ok_or_else(|| {
                    RenderError::DanglingAttachment {
                        message_index: mi,
                        block_index: bi,
                        attachment_id: attachment_id.clone(),
                    }
                })?;
                out.push_str(MEDIA_MARKER);
                attachments.push(*att);
                // resolve() already verified the attachment kind
                // matches the content-block variant (e.g. an Image
                // block resolves to an Attachment::Image).
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
                tool_call_id,
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
                let tool_name = tool_name_by_call_id
                    .get(tool_call_id)
                    .copied()
                    .or_else(|| guess_tool_name_from_tools(tools));
                if let Some(name) = tool_name {
                    out.push_str("response:");
                    out.push_str(name);
                    out.push('{');
                    render_text_only_response(out, content);
                    out.push('}');
                } else {
                    // Couldn't pair to any ToolUse and tools[] is
                    // ambiguous — emit raw content. Gemma will treat
                    // this as freeform tool output; worse than a
                    // perfect render but doesn't crash.
                    render_text_only_response(out, content);
                }
                out.push_str("<tool_response|>");
            }
            ContentBlock::Unknown => {
                return Err(RenderError::UnknownBlock {
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

/// Last-ditch fallback when a `ToolResult` cannot be paired to any
/// `ToolUse` via `tool_call_id`. If `tools[]` has exactly one entry
/// we assume it's that one; otherwise return None and the caller
/// emits raw content. Real consumers always send the matching
/// `tool_call_id` so this branch should be dead in practice.
fn guess_tool_name_from_tools(tools: &[Tool]) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use super::super::tool_grammar::check_rule_closure;
    use super::*;
    use serde_json::json;

    fn tools(names: &[&str]) -> Vec<Tool> {
        names
            .iter()
            .map(|n| Tool {
                name: (*n).to_string(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
            })
            .collect()
    }

    fn grammar_for(choice: ToolChoice, names: &[&str]) -> ToolGrammar {
        Gemma4Renderer::new()
            .tool_call_grammar(choice, &tools(names))
            .expect("gemma4 supports every valid mode")
            .expect("every valid mode constrains something")
    }

    #[test]
    fn every_mode_emits_a_closed_grammar() {
        // The check that would otherwise only run at generation time,
        // on the Tier-3 path that no default gate exercises.
        for choice in [ToolChoice::Auto, ToolChoice::Required, ToolChoice::None] {
            let g = grammar_for(choice, &["get_weather", "send_email"]);
            assert_eq!(
                check_rule_closure(&g.gbnf, "root"),
                Ok(()),
                "{choice:?} grammar:\n{}",
                g.gbnf
            );
        }
    }

    #[test]
    fn required_is_eager_and_auto_is_lazy() {
        // The whole distinction between a guarantee and a hint. A lazy
        // `required` could not force anything, because a grammar masks
        // no tokens until it triggers.
        let required = grammar_for(ToolChoice::Required, &["f"]);
        assert!(!required.is_lazy(), "required must constrain from token 0");

        let auto = grammar_for(ToolChoice::Auto, &["f"]);
        assert!(auto.is_lazy(), "auto must not constrain ordinary prose");
        assert_eq!(auto.triggers, vec![r"<\|tool_call>".to_string()]);
    }

    #[test]
    fn required_root_demands_a_call_after_optional_content() {
        let g = grammar_for(ToolChoice::Required, &["f"]);
        // `prefix-0` is nullable and `tool-call` is not, which is what
        // keeps every EOG token masked until the closer is emitted.
        assert!(
            g.gbnf.starts_with("root ::= prefix-0 tool-call\n"),
            "{}",
            g.gbnf
        );
        assert!(!g.gbnf.contains("tool-call?"), "{}", g.gbnf);
    }

    #[test]
    fn auto_root_starts_at_the_opener() {
        // Lazy grammars are fed output from the trigger match onward, so
        // a content prefix rule would be wrong here, not merely
        // redundant: it would let the replayed opener be consumed as
        // content.
        let g = grammar_for(ToolChoice::Auto, &["f"]);
        assert!(g.gbnf.starts_with("root ::= tool-call\n"), "{}", g.gbnf);
        assert!(!g.gbnf.contains("prefix-"), "{}", g.gbnf);
    }

    #[test]
    fn the_call_body_alternates_over_exactly_the_declared_tools() {
        let g = grammar_for(ToolChoice::Required, &["get_weather", "send_email"]);
        let body = g
            .gbnf
            .lines()
            .find(|l| l.starts_with("tool-body ::="))
            .expect("tool-body rule");
        assert_eq!(
            body,
            "tool-body ::= \"get_weather\" gemma4-dict | \"send_email\" gemma4-dict"
        );
    }

    #[test]
    fn the_call_wrapper_is_gemma_syntax_not_json() {
        // Issue #38 proposed reusing `json_schema_to_gbnf`; this is the
        // syntax that makes that impossible.
        let g = grammar_for(ToolChoice::Required, &["f"]);
        assert!(
            g.gbnf
                .contains("tool-call ::= \"<|tool_call>call:\" tool-body \"<tool_call|>\""),
            "{}",
            g.gbnf
        );
        assert!(
            g.gbnf.contains("gemma4-string ::= \"<|\\\"|>\""),
            "{}",
            g.gbnf
        );
    }

    #[test]
    fn a_tool_name_cannot_break_out_of_its_literal() {
        // Names arrive from the request. An unescaped `"` would close
        // the literal and let the rest of the name be read as grammar.
        let g = grammar_for(ToolChoice::Required, &["ev\"il"]);
        assert!(g.gbnf.contains("\"ev\\\"il\" gemma4-dict"), "{}", g.gbnf);
        assert_eq!(check_rule_closure(&g.gbnf, "root"), Ok(()), "{}", g.gbnf);
    }

    #[test]
    fn none_forbids_the_opener_and_triggers_on_nothing() {
        let g = grammar_for(ToolChoice::None, &["f"]);
        assert!(!g.is_lazy(), "a trigger would defeat the point of none");
        assert!(g.gbnf.starts_with("root ::= no-tool-0\n"), "{}", g.gbnf);
        // No call syntax at all: the language is "output without a call".
        assert!(!g.gbnf.contains("tool-body"), "{}", g.gbnf);
        assert!(!g.gbnf.contains("call:"), "{}", g.gbnf);
    }

    #[test]
    fn the_key_rule_matches_what_the_parser_can_quote() {
        // Narrower than upstream's `[^:}]+` on purpose: `quote_bare_keys`
        // only quotes `[A-Za-z][A-Za-z0-9_]*`, so a wider grammar would
        // admit bodies our own parser reports as Malformed.
        let g = grammar_for(ToolChoice::Required, &["f"]);
        assert!(
            g.gbnf
                .contains("gemma4-dict-key ::= [a-zA-Z] [a-zA-Z0-9_]* \":\""),
            "{}",
            g.gbnf
        );
        assert!(!g.gbnf.contains("[^:}]"), "{}", g.gbnf);
    }

    #[test]
    fn string_content_excludes_what_the_fence_substitution_would_break() {
        // The parser rewrites `<|"|>` to `"` and hands the result to
        // serde_json, so a literal quote or backslash inside the content
        // corrupts the JSON it parses.
        let g = grammar_for(ToolChoice::Required, &["f"]);
        assert!(
            g.gbnf.contains(r#"str-safe ::= [^\"\\\x00-\x1f]"#),
            "{}",
            g.gbnf
        );
    }

    #[test]
    fn interstitial_whitespace_is_only_space_and_tab() {
        // `quote_bare_keys` skips exactly space and tab when looking back
        // for a key boundary, so permitting a newline there would produce
        // an unquoted, unparseable key.
        let g = grammar_for(ToolChoice::Required, &["f"]);
        assert!(g.gbnf.contains(r"ws ::= [ \t]*"), "{}", g.gbnf);
    }

    #[test]
    fn an_unknown_mode_is_refused_rather_than_treated_as_auto() {
        // `resolve()` rejects this first; if it ever reaches a renderer,
        // silently downgrading it would restore the fail-open that the
        // wire-level check exists to close.
        let err = Gemma4Renderer::new()
            .tool_call_grammar(ToolChoice::Unknown, &tools(&["f"]))
            .expect_err("unknown must not produce a grammar");
        assert!(matches!(err, RenderError::Unsupported { .. }));
    }

    #[test]
    fn granite_refuses_every_mode() {
        // The trait default. A family whose prompt cannot express tool
        // calls must not accept a tool_choice and ignore it.
        use super::super::GraniteRenderer;
        for choice in [ToolChoice::Auto, ToolChoice::Required, ToolChoice::None] {
            let err = GraniteRenderer::new()
                .tool_call_grammar(choice, &tools(&["f"]))
                .expect_err("granite has no tool-call syntax");
            assert!(
                matches!(
                    err,
                    RenderError::Unsupported {
                        family: ChatFamily::Granite,
                        ..
                    }
                ),
                "got: {err}"
            );
        }
    }
}
