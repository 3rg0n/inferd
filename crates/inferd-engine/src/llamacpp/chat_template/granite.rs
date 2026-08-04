//! IBM Granite prompt-format renderer.
//!
//! The second implementor of [`ChatRenderer`] (ADR 0026 §Decision
//! part 4) and the family that motivated the registry:
//! `ibm-granite/granite-docling-258M`, whose text tower converts to
//! GGUF architecture `"llama"` — byte-identical to Llama-3-Instruct,
//! whose grammar is nothing like this one.
//!
//! ## Format
//!
//! Transcribed from `granite-docling-258M/chat_template.jinja` and
//! corroborated by llama.cpp's own `LLM_CHAT_TEMPLATE_GRANITE_3_X`
//! (`src/llama-chat.cpp:631`). Per message:
//!
//! ```text
//! <|start_of_role|>{role}<|end_of_role|>{content}<|end_of_text|>
//! ```
//!
//! — each terminated by a literal newline — followed by the
//! generation prompt:
//!
//! ```text
//! <|start_of_role|>assistant<|end_of_role|>
//! ```
//!
//! Three ways this differs from Gemma 4, each verified against the
//! upstream template rather than assumed:
//!
//! - **No literal BOS.** Gemma's renderer emits the `<bos>` string;
//!   Granite's template emits none. The tokenizer still adds the BOS
//!   token (`add_special = true` on both prefill paths), so emitting
//!   one here would double it.
//! - **No system-turn special case.** A system message is just another
//!   `<|start_of_role|>system<|end_of_role|>` turn. There is no
//!   synthesised turn, and no `<|think|>` equivalent.
//! - **No tool grammar.** The template carries no tool-declaration or
//!   tool-call syntax at all. A request carrying `tools[]`, a
//!   `ToolUse` block, or a `ToolResult` block is therefore
//!   **rejected** ([`RenderError::Unsupported`]) rather than rendered
//!   with the tools silently dropped — dropping them would leave the
//!   model answering fluently while never having been told the tools
//!   exist, which is precisely the failure mode ADR 0026 exists to
//!   prevent.
//!
//! Media blocks emit [`MEDIA_MARKER`], exactly as every family does.
//! Granite-docling's own template writes a literal `<image>` token,
//! but that token is the *jinja* template's business: on the mtmd path
//! the fence tokens come from `MTMD_SLICE_TMPL_IDEFICS3`
//! (`tools/mtmd/mtmd.cpp:490`) and are substituted for the marker by
//! `mtmd_tokenize`. Emitting `<image>` here would produce it twice.

use super::{ChatFamily, ChatRenderer, MEDIA_MARKER, RenderError, Rendered};
use inferd_proto::v2::{Attachment, ContentBlock, ResolvedV2, RoleV2};

/// Stateless Granite renderer. Construct with
/// [`GraniteRenderer::new`] and call [`ChatRenderer::render`] per
/// request.
#[derive(Debug, Default)]
pub struct GraniteRenderer;

impl GraniteRenderer {
    /// Construct a renderer.
    pub fn new() -> Self {
        Self
    }
}

impl ChatRenderer for GraniteRenderer {
    fn family(&self) -> ChatFamily {
        ChatFamily::Granite
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        false
    }

    fn render<'a>(&self, resolved: &'a ResolvedV2) -> Result<Rendered<'a>, RenderError> {
        if !resolved.tools.is_empty() {
            return Err(RenderError::Unsupported {
                family: ChatFamily::Granite,
                feature: "tool declarations",
            });
        }
        // Granite has no `<|think|>` equivalent, so an explicit
        // `thinking: true` cannot be honoured. `thinking: false` and
        // an absent field are both satisfiable by rendering nothing.
        if resolved.thinking == Some(true) {
            return Err(RenderError::Unsupported {
                family: ChatFamily::Granite,
                feature: "thinking mode",
            });
        }

        let mut prompt = String::with_capacity(512);
        let mut attachments: Vec<&Attachment> = Vec::new();

        let by_id: std::collections::HashMap<&str, &Attachment> =
            resolved.attachments.iter().map(|a| (a.id(), a)).collect();

        for (mi, msg) in resolved.messages.iter().enumerate() {
            prompt.push_str("<|start_of_role|>");
            prompt.push_str(role_name(msg.role));
            prompt.push_str("<|end_of_role|>");

            for (bi, block) in msg.content.iter().enumerate() {
                match block {
                    ContentBlock::Text { text } => prompt.push_str(text),
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
                        prompt.push_str(MEDIA_MARKER);
                        attachments.push(*att);
                    }
                    ContentBlock::ToolUse { .. } => {
                        return Err(RenderError::Unsupported {
                            family: ChatFamily::Granite,
                            feature: "tool_use content blocks",
                        });
                    }
                    ContentBlock::ToolResult { .. } => {
                        return Err(RenderError::Unsupported {
                            family: ChatFamily::Granite,
                            feature: "tool_result content blocks",
                        });
                    }
                    ContentBlock::Unknown => {
                        return Err(RenderError::UnknownBlock {
                            message_index: mi,
                            block_index: bi,
                        });
                    }
                }
            }

            prompt.push_str("<|end_of_text|>\n");
        }

        // Generation prompt. The upstream template also allows a
        // `controls | tojson()` blob between the role and
        // `<|end_of_role|>`; the v2 wire carries no equivalent field,
        // so it is omitted — which is the template's own behaviour
        // when `controls` is unset.
        prompt.push_str("<|start_of_role|>assistant<|end_of_role|>");

        Ok(Rendered {
            prompt,
            attachments,
        })
    }
}

/// Granite names the assistant turn "assistant" (unlike Gemma, which
/// calls it "model"), so the v2 role names map straight through.
fn role_name(role: RoleV2) -> &'static str {
    match role {
        RoleV2::System => "system",
        RoleV2::User => "user",
        RoleV2::Assistant => "assistant",
    }
}
