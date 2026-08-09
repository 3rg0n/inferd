//! Chat-template rendering for v2 requests.
//!
//! Per ADR 0013: the daemon (the gateway) is responsible for shaping a
//! `ResolvedV2` into the engine-shaped representation the loaded model
//! expects. For the llama.cpp backend that representation is a flat
//! prompt string carrying the model's own control tokens, plus an
//! ordered slice of attachments so libmtmd's `mtmd_tokenize` can splice
//! in the matching bitmaps.
//!
//! Per ADR 0026 that shaping is a **registry keyed to model family**,
//! not a single hardcoded renderer:
//!
//! - [`ChatRenderer`] is the seam. One implementor per family.
//! - [`ChatFamily`] is the key, resolved **once at model load** — from
//!   the operator's explicit `chat_template` config field when set,
//!   otherwise by [`detect_family`] from GGUF metadata.
//! - When neither declares nor detects a family for a model that
//!   *has* a chat template, the load fails. There is no fallback
//!   renderer: rendering one model's turn markers around another
//!   model's prompt produces fluent, confidently wrong output, which
//!   is strictly worse than refusing to start (the same reasoning as
//!   ADR 0025 on audio sample rates).
//!
//! ## Why hand-written renderers
//!
//! `llama_chat_apply_template` "does not use a jinja parser. It only
//! support a pre-defined list of templates" (`include/llama.h:1178`),
//! so delegating is not an option for a model outside that list, and
//! evaluating the GGUF's own jinja `tokenizer.chat_template` inside the
//! daemon would mean interpreting arbitrary program text carried in a
//! store blob (ADR 0026 §Alternatives). Each family is transcribed by
//! hand from its upstream template and pinned by byte-exact tests.
//!
//! ## What is *not* per-family
//!
//! [`MEDIA_MARKER`] is mtmd's default marker, not any model's token.
//! Every renderer emits it for an image / audio / video block and
//! `mtmd_tokenize` substitutes the per-model fence tokens (Gemma's
//! `<start_of_image>`, Idefics3's slice template, …). The media path is
//! model-agnostic and stays that way.

mod gemma4;
mod granite;
mod tool_grammar;

pub use gemma4::Gemma4Renderer;
pub use granite::GraniteRenderer;
pub use tool_grammar::{GRAMMAR_ROOT, ToolGrammar};

use inferd_proto::v2::{Attachment, ResolvedV2, Tool, ToolChoice};

/// The mtmd default media marker. A renderer emits this substring in
/// place of an image / audio / video content block; `mtmd_tokenize`
/// replaces it with the per-modality fence tokens for the associated
/// bitmap. Not a model-specific token — see the module docs.
pub const MEDIA_MARKER: &str = "<__media__>";

/// The GGUF metadata key naming the model's tensor topology. Read as
/// one half of family detection; see [`detect_family`] for why it is
/// not sufficient alone.
pub const GGUF_KEY_ARCHITECTURE: &str = "general.architecture";

/// The GGUF metadata key carrying the model's own jinja chat template.
/// inferd never *evaluates* this (ADR 0026) — it fingerprints the
/// control tokens in it to identify the family.
pub const GGUF_KEY_CHAT_TEMPLATE: &str = "tokenizer.chat_template";

/// Output of [`ChatRenderer::render`].
///
/// `prompt` is the flat string ready for `mtmd_tokenize`.
/// `attachments` lists the attachments referenced by media markers in
/// `prompt`, in the order the markers appear. The engine adapter
/// supplies them to `mtmd_tokenize` in that same order so each marker
/// resolves to the correct bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered<'a> {
    /// Flat prompt with control tokens + media markers.
    pub prompt: String,
    /// Attachments in the order their content blocks appear.
    pub attachments: Vec<&'a Attachment>,
}

/// Errors from [`ChatRenderer::render`].
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
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
    /// The request asked for something this family's prompt grammar
    /// has no representation for (e.g. tool declarations against a
    /// family whose template carries none).
    ///
    /// Rendering it anyway would mean silently dropping the request's
    /// intent, which is the failure mode ADR 0026 exists to prevent —
    /// the model would answer fluently, having never been told about
    /// the tools.
    #[error("{family} prompt grammar does not support {feature}")]
    Unsupported {
        /// The family whose renderer refused.
        family: ChatFamily,
        /// The request feature it has no grammar for.
        feature: &'static str,
    },
}

/// A model family whose prompt grammar inferd can render.
///
/// This is the registry key (ADR 0026). It identifies the *prompt
/// grammar*, which is not the same thing as `general.architecture`
/// (the tensor topology) — see [`detect_family`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatFamily {
    /// Gemma 4 (`<|turn>role … <turn|>`), the reference model.
    Gemma4,
    /// IBM Granite (`<|start_of_role|>role<|end_of_role|> …
    /// <|end_of_text|>`). Covers granite-docling, whose text tower
    /// reports `general.architecture == "llama"` but whose chat
    /// grammar is Granite's.
    Granite,
}

impl ChatFamily {
    /// Every family name accepted by the `chat_template` config field,
    /// for error messages and config validation.
    pub const NAMES: &'static [&'static str] = &["gemma4", "granite"];

    /// Stable lowercase identifier. This is the string an operator
    /// writes in the `chat_template` config field, so it is a config
    /// contract: do not rename a variant's string.
    pub fn as_str(self) -> &'static str {
        match self {
            ChatFamily::Gemma4 => "gemma4",
            ChatFamily::Granite => "granite",
        }
    }

    /// Parse an operator-supplied `chat_template` config value.
    /// Case-insensitive; `None` for anything unrecognised so the
    /// caller can name the valid set in its error.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gemma4" => Some(ChatFamily::Gemma4),
            "granite" => Some(ChatFamily::Granite),
            _ => None,
        }
    }

    /// Build the renderer for this family.
    pub fn renderer(self) -> Box<dyn ChatRenderer> {
        match self {
            ChatFamily::Gemma4 => Box::new(Gemma4Renderer::new()),
            ChatFamily::Granite => Box::new(GraniteRenderer::new()),
        }
    }
}

impl std::fmt::Display for ChatFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Renders a `ResolvedV2` into one model family's flat prompt.
///
/// Implementors are stateless and shared across requests, so the trait
/// requires `Send + Sync`: the adapter resolves one renderer at model
/// load and every generation borrows it.
pub trait ChatRenderer: Send + Sync + std::fmt::Debug {
    /// Which family this renderer speaks. Reported in diagnostics so a
    /// wrong-family install is one log line to diagnose.
    fn family(&self) -> ChatFamily;

    /// Render `resolved` into a flat prompt + the ordered list of
    /// attachments its media markers refer to.
    fn render<'a>(&self, resolved: &'a ResolvedV2) -> Result<Rendered<'a>, RenderError>;

    /// Whether this family's prompt grammar can express tool
    /// declarations and tool calls. Surfaced on the admin
    /// `capabilities` frame so a consumer learns the answer before
    /// sending, rather than from a rejected request.
    fn supports_tools(&self) -> bool {
        true
    }

    /// Whether this family's prompt grammar can activate a separated
    /// reasoning trace. Surfaced on the `capabilities` frame for the
    /// same reason as [`Self::supports_tools`].
    fn supports_thinking(&self) -> bool {
        true
    }

    /// Build the constrained-decoding grammar that makes `choice` a
    /// *guarantee* for this family's tool-call syntax, or `None` when
    /// the mode needs no constraint.
    ///
    /// This is the enforcement half of `tool_choice`. Declaring tools
    /// in the prompt is a request the model may ignore; without a
    /// grammar, `required` would be a promise the daemon does not
    /// keep — the model is free to answer in prose and the caller has
    /// no way to tell that its constraint was dropped. That silent
    /// fail-open is the same class of bug ADR 0025 refuses for audio
    /// sample rates and ADR 0026 refuses for prompt families, so a
    /// family that cannot enforce a mode must return
    /// [`RenderError::Unsupported`] rather than an empty grammar.
    ///
    /// `tools` is non-empty whenever `choice` is `Some` —
    /// `RequestV2::resolve` rejects `tool_choice` without tools, so
    /// there is no "constrain to nothing" case to model here.
    ///
    /// The default refuses every mode, so a new family opts in
    /// deliberately: inheriting a silently-unenforced `required` is
    /// exactly the outcome this method exists to prevent.
    fn tool_call_grammar(
        &self,
        choice: ToolChoice,
        _tools: &[Tool],
    ) -> Result<Option<ToolGrammar>, RenderError> {
        let _ = choice;
        Err(RenderError::Unsupported {
            family: self.family(),
            feature: "tool_choice",
        })
    }
}

/// Identify the prompt grammar of a loaded model from its GGUF
/// metadata.
///
/// `arch` is `general.architecture`; `chat_template` is the raw jinja
/// in `tokenizer.chat_template`. Returns `None` when the pair matches
/// no known family — the caller then fails the load rather than
/// guessing (ADR 0026 §Decision part 3).
///
/// ## Why the template fingerprint is the decisive half
///
/// `general.architecture` alone cannot key this registry.
/// granite-docling's text tower converts to architecture `"llama"`
/// (its `config.json` reports `text_config.model_type: "llama"`) —
/// byte-identical to what a Llama-3-Instruct GGUF reports, and their
/// prompt formats share nothing. Architecture identifies the tensor
/// topology the *engine* needs; the control tokens in the template
/// identify the grammar the *renderer* needs. So each rule below keys
/// on a control-token fingerprint, and consults `arch` only to
/// corroborate a match that the fingerprint already made.
///
/// A model with no `tokenizer.chat_template` at all is not a chat
/// model (an embedding or base model), and yields `None` with nothing
/// lost — there is no grammar to get wrong. The caller distinguishes
/// that case from "has a template we cannot render", which is the
/// correctness hole.
pub fn detect_family(arch: Option<&str>, chat_template: Option<&str>) -> Option<ChatFamily> {
    let template = chat_template?;

    // Gemma 4: `<|turn>system … <turn|>`. Corroborated by the
    // `gemma4` / `gemma4-assistant` architecture strings
    // (`gguf-py/gguf/constants.py:1038`), which are unambiguous —
    // unlike `llama` — so a mismatch here means a retuned template we
    // have not transcribed, and refusing is correct.
    if template.contains("<|turn>")
        && arch.is_some_and(|a| a.starts_with("gemma4"))
        && template.contains("<turn|>")
    {
        return Some(ChatFamily::Gemma4);
    }

    // Granite: `<|start_of_role|>role<|end_of_role|>`. Decisive on
    // its own — no other family in the registry emits these tokens —
    // which is what lets granite-docling be told apart from Llama-3
    // despite both reporting architecture `llama`.
    if template.contains("<|start_of_role|>") && template.contains("<|end_of_role|>") {
        return Some(ChatFamily::Granite);
    }

    None
}

/// A short, log-safe description of a chat template that matched no
/// family, for the load-failure message.
///
/// Deliberately not the template itself: real templates run to tens of
/// kilobytes of jinja (Gemma 4's is ~16 KB), which would bury the
/// error. Reports the length plus the leading run of the template,
/// which is where the distinguishing control tokens live.
pub fn template_fingerprint(chat_template: &str) -> String {
    const HEAD: usize = 96;
    let head: String = chat_template
        .chars()
        .take(HEAD)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let ellipsis = if chat_template.chars().nth(HEAD).is_some() {
        "…"
    } else {
        ""
    };
    format!(
        "{} chars, starts {:?}{}",
        chat_template.len(),
        head,
        ellipsis
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_strings_round_trip() {
        for name in ChatFamily::NAMES {
            let f = ChatFamily::parse(name).expect("NAMES entry must parse");
            assert_eq!(f.as_str(), *name);
        }
    }

    #[test]
    fn family_parse_is_case_and_space_insensitive() {
        assert_eq!(ChatFamily::parse(" Gemma4 "), Some(ChatFamily::Gemma4));
        assert_eq!(ChatFamily::parse("GRANITE"), Some(ChatFamily::Granite));
        assert_eq!(ChatFamily::parse("llama3"), None);
    }

    #[test]
    fn renderer_matches_its_family() {
        for name in ChatFamily::NAMES {
            let f = ChatFamily::parse(name).unwrap();
            assert_eq!(f.renderer().family(), f);
        }
    }

    #[test]
    fn detects_gemma4_from_turn_tokens() {
        // Shape lifted from the shipped Gemma 4 E4B GGUF template.
        let tmpl = "{{- '<|turn>system\n' -}}{{- '<turn|>\n' -}}";
        assert_eq!(
            detect_family(Some("gemma4"), Some(tmpl)),
            Some(ChatFamily::Gemma4)
        );
        assert_eq!(
            detect_family(Some("gemma4-assistant"), Some(tmpl)),
            Some(ChatFamily::Gemma4)
        );
    }

    #[test]
    fn detects_granite_from_role_tokens() {
        // Shape lifted from granite-docling-258M's chat_template.jinja.
        let tmpl = "{{- '<|start_of_role|>' + message['role'] + '<|end_of_role|>' -}}";
        assert_eq!(
            detect_family(Some("llama"), Some(tmpl)),
            Some(ChatFamily::Granite)
        );
        // Same grammar, and the architecture string is not consulted —
        // which is the point: `llama` is shared with Llama-3.
        assert_eq!(
            detect_family(Some("granite"), Some(tmpl)),
            Some(ChatFamily::Granite)
        );
    }

    #[test]
    fn llama3_style_template_is_not_mistaken_for_granite() {
        // The collision ADR 0026 exists to prevent: identical
        // architecture, unrelated grammar. Must NOT resolve.
        let tmpl = "<|start_header_id|>{{ role }}<|end_header_id|>\n\n{{ content }}<|eot_id|>";
        assert_eq!(detect_family(Some("llama"), Some(tmpl)), None);
    }

    #[test]
    fn gemma_arch_with_foreign_template_does_not_resolve() {
        // Architecture says gemma4 but the grammar is someone else's:
        // refuse rather than render Gemma markers around it.
        let tmpl = "<|im_start|>{{ role }}\n{{ content }}<|im_end|>";
        assert_eq!(detect_family(Some("gemma4"), Some(tmpl)), None);
    }

    #[test]
    fn absent_template_yields_none() {
        // An embedding model (embeddinggemma-300m reports
        // architecture `gemma-embedding` and carries no chat
        // template). Nothing to render, nothing lost.
        assert_eq!(detect_family(Some("gemma-embedding"), None), None);
        assert_eq!(detect_family(None, None), None);
    }

    #[test]
    fn fingerprint_is_bounded_and_control_free() {
        let long = format!("{}\n{}", "<|weird|>", "x".repeat(4096));
        let fp = template_fingerprint(&long);
        assert!(fp.contains("4106 chars"), "got: {fp}");
        assert!(fp.contains("<|weird|>"));
        assert!(fp.ends_with('…'));
        // The raw newline must not survive into a log line.
        assert!(!fp.contains('\n'));
        assert!(fp.len() < 200);
    }

    #[test]
    fn fingerprint_of_short_template_has_no_ellipsis() {
        let fp = template_fingerprint("<|start_of_role|>");
        assert!(fp.starts_with("17 chars"));
        assert!(!fp.ends_with('…'));
    }
}
