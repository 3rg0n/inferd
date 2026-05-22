//! Chat-template rendering for v2 requests.
//!
//! Per ADR 0013: the daemon (the gateway) is responsible for shaping
//! a `ResolvedV2` into the engine-shaped representation each backend
//! expects. For the v0.2 default backend (llama.cpp + Gemma 4), that
//! representation is a flat prompt string with Gemma 4's control
//! tokens (`<|turn>...<turn|>`, `<|tool>...<tool|>`,
//! `<|tool_call>...<tool_call|>`, `<|tool_response>...<tool_response|>`,
//! `<__media__>` markers for image/audio attachments) plus an
//! ordered slice of attachments so libmtmd's `mtmd_tokenize` can
//! splice in the matching bitmaps.
//!
//! Cloud backends (Phase 5A onward) get their own renderer that
//! translates `ResolvedV2` into HTTP request bodies — the typed
//! content blocks survive the wire round-trip; only the local-engine
//! adapter has to flatten.
//!
//! This module ships *renderers only*. It does not call libllama; it
//! does not reach into the engine. Phase 3A will wire it into the
//! `LlamaCpp` adapter's `generate_v2` impl.

mod gemma4;

pub use gemma4::{Gemma4RenderError, Gemma4Rendered, Gemma4Renderer};
