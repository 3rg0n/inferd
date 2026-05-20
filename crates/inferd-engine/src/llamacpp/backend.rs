//! `LlamaCpp` adapter — implements `Backend` against the FFI bindings.
//!
//! Lifecycle:
//! 1. `LlamaCpp::new` calls `llama_backend_init` (process-wide; idempotent
//!    via a `Once`), loads + verifies the model, allocates a
//!    `llama_context` with a configurable `n_ctx`, flips `ready`.
//! 2. `generate` builds the prompt by applying the model's chat template
//!    to the request's messages, tokenizes, runs the decode loop on a
//!    `spawn_blocking` task, and streams `TokenEvent`s through an
//!    `mpsc::channel`.
//! 3. The blocking task checks `tx.blocking_send` each iteration; when
//!    the receiver is dropped (caller cancels), `blocking_send` errors
//!    and the loop exits cleanly. On exit, the per-request KV cache is
//!    cleared via `llama_memory_clear` so the next request starts fresh.
//!
//! Concurrency: the daemon's admission queue serialises generations
//! (1 active in v0.1), so a single shared `llama_context` is sufficient.
//! v0.2 may revisit this if multi-active backends land.

#![allow(unsafe_code)] // FFI surface; module-scoped.

use crate::backend::{
    AcceleratorInfo, AcceleratorKind, Backend, BackendCapabilities, GenerateError, TokenEvent,
    TokenEventV2, TokenStream, TokenStreamV2,
};
use crate::ffi;
use crate::llamacpp::chat_template::Gemma4Renderer;
use crate::llamacpp::loader::{ModelHandle, ModelLoadError, load_model};
use crate::llamacpp::mtmd::{Bitmap, Mtmd, MtmdConfig, MtmdError};
use crate::llamacpp::tool_parser::{Output as TokenOutput, ToolCallParser};
use async_trait::async_trait;
use base64::Engine as _;
use inferd_proto::v2::{Attachment, ResolvedV2, StopReasonV2, UsageV2};
use inferd_proto::{Resolved, StopReason, Usage};
use std::ffi::CString;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

static LLAMA_BACKEND_INIT: Once = Once::new();

/// Errors specific to the `LlamaCpp` adapter.
#[derive(Debug, thiserror::Error)]
pub enum LlamaCppError {
    /// Model file load failed.
    #[error("load: {0}")]
    Load(#[from] ModelLoadError),
    /// `llama_init_from_model` returned null.
    #[error("llama_init_from_model returned null")]
    ContextInit,
    /// Sampler chain construction failed.
    #[error("sampler chain init failed")]
    Sampler,
    /// Tokeniser failed on the prompt string.
    #[error("tokenize failed")]
    Tokenize,
    /// `llama_decode` returned a non-zero error code.
    #[error("llama_decode failed: {0}")]
    Decode(i32),
    /// libmtmd initialisation or eval-chunk error.
    #[error("mtmd: {0}")]
    Mtmd(#[from] MtmdError),
    /// v2 request used an attachment-like content block but the
    /// adapter was constructed without an mmproj.
    #[error("v2 request requires mmproj but none was configured")]
    NoMmproj,
    /// Chat-template renderer failed (e.g. unknown content-block).
    #[error("chat template: {0}")]
    Render(String),
    /// base64-decoding an attachment's `bytes` field failed.
    #[error("attachment base64 decode failed for {0:?}")]
    Base64(String),
}

impl From<LlamaCppError> for GenerateError {
    fn from(e: LlamaCppError) -> Self {
        GenerateError::Internal(e.to_string())
    }
}

/// Configuration for `LlamaCpp::new`.
#[derive(Debug, Clone)]
pub struct LlamaCppConfig {
    /// GGUF model file path.
    pub model_path: std::path::PathBuf,
    /// Optional expected SHA-256 of the model file (THREAT_MODEL F-5).
    pub model_sha256: Option<[u8; 32]>,
    /// Context window in tokens.
    pub n_ctx: u32,
    /// Layers to offload to GPU; 0 = CPU-only.
    pub n_gpu_layers: i32,
    /// Sampler RNG seed.
    pub seed: u32,
    /// Optional mmproj (multimodal projector) file path. When set,
    /// the adapter constructs a `Mtmd` context against this file +
    /// the loaded text model, advertises matching capabilities, and
    /// can serve v2 requests with image / audio attachments.
    pub mmproj_path: Option<std::path::PathBuf>,
    /// Optional expected SHA-256 of the mmproj file. Same shape as
    /// `model_sha256`; verified before mtmd_init_from_file.
    pub mmproj_sha256: Option<[u8; 32]>,
}

impl Default for LlamaCppConfig {
    fn default() -> Self {
        Self {
            model_path: std::path::PathBuf::new(),
            model_sha256: None,
            n_ctx: 8192,
            n_gpu_layers: 0,
            seed: 0xDEADBEEF,
            mmproj_path: None,
            mmproj_sha256: None,
        }
    }
}

/// Owned `llama_context`. `Drop` runs `llama_free`.
struct ContextHandle {
    ptr: NonNull<ffi::llama_context>,
}

// SAFETY: see `ModelHandle` — internal sync for read ops, exclusive Drop.
unsafe impl Send for ContextHandle {}
unsafe impl Sync for ContextHandle {}

impl Drop for ContextHandle {
    fn drop(&mut self) {
        // SAFETY: pointer was returned by `llama_init_from_model` and
        // not freed yet.
        unsafe { ffi::llama_free(self.ptr.as_ptr()) };
    }
}

/// `LlamaCpp` backend adapter.
pub struct LlamaCpp {
    name: &'static str,
    ready: AtomicBool,
    seed: u32,
    /// Hardware-acceleration snapshot (compile-time GGML backend +
    /// configured `n_gpu_layers`). Cached on the adapter so
    /// `capabilities()` can return it without locking the state mutex.
    accelerator: AcceleratorInfo,
    /// Shared so the spawn_blocking generation task can reach the model
    /// and context. Locked for the duration of one generation; the
    /// daemon's queue serialises calls, so contention is structural
    /// (always 1 holder + 0 waiters in v0.1).
    state: Arc<Mutex<State>>,
}

/// Compile-time GGML backend the engine was built against. Reflects
/// the cargo features active at build time (`cuda` / `metal` /
/// `vulkan` / `rocm` — at most one is meaningful per build).
const fn compile_time_accelerator_kind() -> AcceleratorKind {
    if cfg!(feature = "cuda") {
        AcceleratorKind::Cuda
    } else if cfg!(feature = "metal") {
        AcceleratorKind::Metal
    } else if cfg!(feature = "vulkan") {
        AcceleratorKind::Vulkan
    } else if cfg!(feature = "rocm") {
        AcceleratorKind::Rocm
    } else {
        AcceleratorKind::Cpu
    }
}

struct State {
    model: ModelHandle,
    ctx: ContextHandle,
    /// Multimodal context. `Some` when the adapter was constructed
    /// with an `mmproj_path`. `Mtmd` borrows the model pointer; the
    /// drop order in `State` (mtmd → ctx → model) ensures it's freed
    /// before the model it depends on.
    mtmd: Option<Mtmd>,
    /// Cached capabilities derived from the `Mtmd` probe. None when
    /// no mmproj was configured (text-only).
    caps_v2: Option<BackendCapabilitiesV2>,
}

/// Internal capability snapshot used by `Backend::capabilities()`.
#[derive(Debug, Clone, Copy)]
struct BackendCapabilitiesV2 {
    vision: bool,
    audio: bool,
    /// Audio sample rate the mmproj's encoder expects, in Hz.
    /// Reported on the admin status surface in a future commit so
    /// middleware can resample before sending. Currently
    /// informational only.
    #[allow(dead_code)]
    audio_sample_rate: Option<u32>,
}

impl LlamaCpp {
    /// Build a new `LlamaCpp` adapter. Performs model load + context
    /// allocation synchronously. `Backend::ready()` returns `true` once
    /// this returns `Ok`.
    pub fn new(config: LlamaCppConfig) -> Result<Self, LlamaCppError> {
        ensure_backend_init();

        let model = load_model(
            &config.model_path,
            config.model_sha256.as_ref(),
            config.n_gpu_layers,
        )?;

        // SAFETY: FFI. `model.as_ptr()` is non-null and valid for the
        // lifetime of `model`. `ctx_params` is POD initialised by libllama.
        let ctx_ptr = unsafe {
            let mut params = ffi::llama_context_default_params();
            params.n_ctx = config.n_ctx;
            ffi::llama_init_from_model(model.as_ptr(), params)
        };

        let ctx = NonNull::new(ctx_ptr)
            .map(|ptr| ContextHandle { ptr })
            .ok_or(LlamaCppError::ContextInit)?;

        // Optional mtmd context for multimodal v2 support.
        let (mtmd, caps_v2) = match config.mmproj_path.as_deref() {
            Some(mmproj) => {
                // Verify mmproj SHA-256 if supplied. Reuses the same
                // F-5 constant-time path as the text model.
                if let Some(expected) = config.mmproj_sha256.as_ref() {
                    crate::llamacpp::loader::verify_mmproj_sha256(mmproj, expected)?;
                }
                // SAFETY: caller (this fn) holds `model` for the
                // entirety of `State`'s lifetime; `Mtmd` lives inside
                // the same `State` struct so its borrow is satisfied.
                let mtmd_ctx = unsafe { Mtmd::new(mmproj, model.as_ptr(), MtmdConfig::default())? };
                let caps = BackendCapabilitiesV2 {
                    vision: mtmd_ctx.supports_vision(),
                    audio: mtmd_ctx.supports_audio(),
                    audio_sample_rate: mtmd_ctx.audio_sample_rate(),
                };
                (Some(mtmd_ctx), Some(caps))
            }
            None => (None, None),
        };

        let accelerator = AcceleratorInfo {
            kind: compile_time_accelerator_kind(),
            gpu_layers: config.n_gpu_layers.max(0) as u32,
        };

        Ok(Self {
            name: "llamacpp",
            ready: AtomicBool::new(true),
            seed: config.seed,
            accelerator,
            state: Arc::new(Mutex::new(State {
                model,
                ctx,
                mtmd,
                caps_v2,
            })),
        })
    }
}

fn ensure_backend_init() {
    LLAMA_BACKEND_INIT.call_once(|| {
        // SAFETY: FFI; documented as required-once at process start.
        unsafe { ffi::llama_backend_init() };
    });
}

#[async_trait]
impl Backend for LlamaCpp {
    fn name(&self) -> &str {
        self.name
    }

    fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Read the cached caps probed at construction. v2 is true
        // only when an mmproj was configured AND the mtmd context
        // initialised successfully — without an mmproj we'd reject
        // image / audio attachments anyway, so v2-without-mmproj is
        // not a useful state to advertise.
        let snap = {
            let guard = self.state.lock().expect("poisoned llamacpp state mutex");
            guard.caps_v2
        };
        match snap {
            Some(caps) => BackendCapabilities {
                v2: true,
                vision: caps.vision,
                audio: caps.audio,
                video: false,
                tools: true,
                thinking: true,
                accelerator: self.accelerator,
            },
            None => BackendCapabilities {
                accelerator: self.accelerator,
                ..BackendCapabilities::default()
            },
        }
    }

    async fn generate_v2(&self, req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        if !self.ready() {
            return Err(GenerateError::NotReady);
        }

        // Render the prompt + attachment-order on the calling task.
        let renderer = Gemma4Renderer::new();
        let rendered = renderer
            .render(&req)
            .map_err(|e| GenerateError::InvalidRequest(format!("render: {e}")))?;

        // Decode each referenced attachment's bytes into Bitmaps.
        let bitmaps: Vec<Bitmap> = rendered
            .attachments
            .iter()
            .map(|att| build_bitmap(att))
            .collect::<Result<_, _>>()
            .map_err(|e| GenerateError::InvalidRequest(format!("attachment: {e}")))?;

        let prompt = rendered.prompt;
        let max_new = req.max_tokens.unwrap_or(crate::DEFAULT_V2_MAX_TOKENS);

        let (tx, rx) = mpsc::channel(8);
        let state = Arc::clone(&self.state);
        let seed = self.seed;
        let req_clone = req;

        tokio::task::spawn_blocking(move || {
            let outcome =
                run_generation_v2(&state, &prompt, &bitmaps, &req_clone, max_new, seed, &tx);
            if let Err(e) = outcome {
                warn!(error = %e, "v2 generation aborted mid-stream");
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn generate(&self, req: Resolved) -> Result<TokenStream, GenerateError> {
        if !self.ready() {
            return Err(GenerateError::NotReady);
        }

        // Build prompt up-front (chat template + tokenize) on the calling
        // task; this is fast and lets us return InvalidRequest synchronously
        // rather than as a stream-terminal error.
        let prompt = render_chat_template(&self.state, &req.messages)
            .ok_or_else(|| GenerateError::InvalidRequest("chat template render failed".into()))?;

        let (tx, rx) = mpsc::channel(8);
        let state = Arc::clone(&self.state);
        let seed = self.seed;
        let resolved = req;
        let prompt_bytes = prompt;

        tokio::task::spawn_blocking(move || {
            let outcome = run_generation(&state, &prompt_bytes, &resolved, seed, &tx);
            if let Err(e) = outcome {
                // Mid-stream failure surfaces as silent termination — the
                // daemon translates that to Response::Error{code:
                // backend_unavailable}. Logging gives operators something
                // to grep for.
                warn!(error = %e, "generation aborted mid-stream");
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn stop(&self, _timeout: Duration) -> Result<(), GenerateError> {
        // Mark not-ready so any in-flight `generate` calls error before
        // touching the FFI. Drop will free model + context when the
        // adapter itself is dropped.
        self.ready.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// Render messages into Gemma 4's chat-template format, by hand.
///
/// We don't use `llama_chat_apply_template`: it explicitly does not
/// parse Jinja, and Gemma 4's canonical template is shipped as Jinja
/// inside the GGUF metadata. llama.cpp does have a fallback table of
/// hand-coded templates keyed by architecture name, but relying on
/// that table to recognise a Gemma quant we vendored is brittle —
/// especially with unsloth's repacks where the architecture string
/// occasionally drifts.
///
/// Gemma 4's format is small and well-known, documented in
/// `docs/protocol-v1.md` and in upstream's `tokenizer_config.json`:
///
/// ```text
/// <start_of_turn>user
/// <user message><end_of_turn>
/// <start_of_turn>model
/// <assistant message — for replay><end_of_turn>
/// <start_of_turn>model
/// ```
///
/// System messages are folded into the first user turn (Gemma 4 has
/// no dedicated system role). The trailing `<start_of_turn>model\n`
/// primes the model to emit the next assistant reply.
fn render_chat_template(
    _state: &Arc<Mutex<State>>,
    messages: &[inferd_proto::Message],
) -> Option<Vec<u8>> {
    use inferd_proto::Role;

    if messages.is_empty() {
        return None;
    }

    // Pre-allocate generously: each turn adds ~30 bytes of boilerplate.
    let mut out = String::with_capacity(
        messages.iter().map(|m| m.content.len()).sum::<usize>() + 64 * messages.len() + 32,
    );

    // Gemma has no system role. If the first message is a system
    // prompt, prepend it to the first user turn we encounter.
    let mut pending_system: Option<&str> = None;
    for m in messages {
        match m.role {
            Role::System => {
                pending_system = Some(m.content.as_str());
            }
            Role::User => {
                out.push_str("<start_of_turn>user\n");
                if let Some(sys) = pending_system.take() {
                    out.push_str(sys);
                    out.push_str("\n\n");
                }
                out.push_str(&m.content);
                out.push_str("<end_of_turn>\n");
            }
            Role::Assistant => {
                out.push_str("<start_of_turn>model\n");
                out.push_str(&m.content);
                out.push_str("<end_of_turn>\n");
            }
        }
    }
    // Prime the next assistant turn.
    out.push_str("<start_of_turn>model\n");

    Some(out.into_bytes())
}

/// Synchronous decode + sample loop. Runs on `spawn_blocking`.
///
/// Errors thrown from here are logged; the receiver sees the channel
/// close with no terminal `Done`, which the daemon translates to an
/// `error` frame per ADR 0007.
fn run_generation(
    state: &Arc<Mutex<State>>,
    prompt: &[u8],
    req: &Resolved,
    seed: u32,
    tx: &mpsc::Sender<TokenEvent>,
) -> Result<(), LlamaCppError> {
    let guard = state.lock().expect("poisoned llamacpp state mutex");
    let model = guard.model.as_ptr();
    let ctx = guard.ctx.ptr.as_ptr();

    // SAFETY: FFI ops on valid pointers held in scope.
    let vocab = unsafe { ffi::llama_model_get_vocab(model) };

    // Tokenize the prompt.
    let prompt_tokens = tokenize(vocab, prompt, true, true)?;

    // Build sampler chain: penalties → grammar (if any) → top-k → top-p →
    // temp → final dist. Order matters; grammar must come before sampling
    // to mask invalid tokens.
    let sampler = build_sampler_chain(vocab, req, seed)?;
    let _sampler_guard = SamplerGuard { ptr: sampler };

    // Reset KV cache so each generation starts clean. v0.1 has no KV
    // sharing across requests — that's a v0.2+ feature.
    // SAFETY: FFI; `ctx` is valid for the lifetime of the lock guard.
    unsafe {
        let mem = ffi::llama_get_memory(ctx);
        if !mem.is_null() {
            ffi::llama_memory_clear(mem, true);
        }
    }

    // Prefill: feed the prompt tokens.
    let mut tokens = prompt_tokens;
    // SAFETY: FFI. `tokens.as_mut_ptr()` valid for `tokens.len()`.
    let mut batch = unsafe { ffi::llama_batch_get_one(tokens.as_mut_ptr(), tokens.len() as i32) };
    let rc = unsafe { ffi::llama_decode(ctx, batch) };
    if rc != 0 {
        return Err(LlamaCppError::Decode(rc));
    }

    let prompt_len = tokens.len() as u32;
    let mut completion_tokens: u32 = 0;
    let max_new = req.max_tokens;

    let mut buf = [0u8; 256];

    for _ in 0..max_new {
        // Sample next token.
        // SAFETY: FFI; `sampler` and `ctx` valid in scope.
        let next: ffi::llama_token = unsafe { ffi::llama_sampler_sample(sampler, ctx, -1) };

        // EOS / EOG detection — clean stop.
        // SAFETY: FFI; `vocab` valid.
        let is_eog = unsafe { ffi::llama_vocab_is_eog(vocab, next) };
        if is_eog {
            let _ = tx.blocking_send(TokenEvent::Done {
                stop_reason: StopReason::End,
                usage: Usage {
                    prompt_tokens: prompt_len,
                    completion_tokens,
                },
            });
            return Ok(());
        }

        // Accept into sampler state (grammar, repetition penalties).
        // SAFETY: FFI; `sampler` valid.
        unsafe { ffi::llama_sampler_accept(sampler, next) };

        // Detokenize → emit Token event.
        let piece = token_to_piece(vocab, next, &mut buf);
        let text = String::from_utf8_lossy(piece).into_owned();
        if tx.blocking_send(TokenEvent::Token(text)).is_err() {
            // Receiver dropped — caller cancelled.
            debug!("generation cancelled (receiver dropped)");
            return Ok(());
        }
        completion_tokens = completion_tokens.saturating_add(1);

        // Feed the new token back for the next forward pass.
        let mut next_arr = [next];
        // SAFETY: FFI; `next_arr` lives for the call.
        batch = unsafe { ffi::llama_batch_get_one(next_arr.as_mut_ptr(), 1) };
        let rc = unsafe { ffi::llama_decode(ctx, batch) };
        if rc != 0 {
            return Err(LlamaCppError::Decode(rc));
        }
    }

    // max_tokens reached cleanly.
    let _ = tx.blocking_send(TokenEvent::Done {
        stop_reason: StopReason::Length,
        usage: Usage {
            prompt_tokens: prompt_len,
            completion_tokens,
        },
    });
    Ok(())
}

/// RAII for the sampler-chain pointer.
struct SamplerGuard {
    ptr: *mut ffi::llama_sampler,
}

unsafe impl Send for SamplerGuard {}

impl Drop for SamplerGuard {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: pointer originates from `llama_sampler_chain_init`
            // and has not been freed.
            unsafe { ffi::llama_sampler_free(self.ptr) };
        }
    }
}

fn build_sampler_chain(
    vocab: *const ffi::llama_vocab,
    req: &Resolved,
    seed: u32,
) -> Result<*mut ffi::llama_sampler, LlamaCppError> {
    // SAFETY: FFI sequence.
    let chain = unsafe {
        let params = ffi::llama_sampler_chain_default_params();
        ffi::llama_sampler_chain_init(params)
    };
    if chain.is_null() {
        return Err(LlamaCppError::Sampler);
    }

    // Grammar first so it can mask tokens before sampling.
    if !req.grammar.is_empty() {
        // F-11: parse-time complexity bound. Reject grammars that are
        // suspiciously large or contain pathologically many
        // alternation operators before we hand them to libllama.
        if let Err(e) = validate_grammar(&req.grammar) {
            unsafe { ffi::llama_sampler_free(chain) };
            return Err(e);
        }

        // SAFETY: `grammar_c` outlives the call; libllama copies the
        // grammar text internally on parse.
        let grammar_c = CString::new(req.grammar.as_bytes()).map_err(|_| LlamaCppError::Sampler)?;
        let root_c = CString::new("root").unwrap();
        let g =
            unsafe { ffi::llama_sampler_init_grammar(vocab, grammar_c.as_ptr(), root_c.as_ptr()) };
        if g.is_null() {
            // Free the chain and bail — bad grammar is a request-level
            // error, surfaces as Internal up the stack but operators see
            // the warning log.
            unsafe { ffi::llama_sampler_free(chain) };
            return Err(LlamaCppError::Sampler);
        }
        unsafe { ffi::llama_sampler_chain_add(chain, g) };
    }

    // Standard chain: top-k → top-p → temp → dist.
    unsafe {
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_k(req.top_k as i32));
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_p(req.top_p as f32, 1));
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_temp(req.temperature as f32));
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_dist(seed));
    }
    Ok(chain)
}

/// Maximum GBNF grammar source length we'll forward to libllama.
/// Real grammars are usually under 4 KB; 64 KB is a generous ceiling
/// that catches obviously-abusive payloads. Codified for F-11.
pub const MAX_GRAMMAR_BYTES: usize = 64 * 1024;

/// Maximum number of alternation operators (`|`) we'll tolerate in a
/// grammar. Each `|` multiplies the search space libllama walks per
/// token; thousands of them in a single grammar is the
/// "exponential alternation" case the threat model calls out.
pub const MAX_GRAMMAR_ALTERNATIONS: usize = 4096;

/// Cheap parse-time complexity check on a GBNF grammar.
///
/// Bounds:
/// - Total length ≤ `MAX_GRAMMAR_BYTES`.
/// - Top-level `|` alternation count ≤ `MAX_GRAMMAR_ALTERNATIONS`
///   (counts every `|` in the source; conservative — `|` inside
///   character classes still counts, which is fine because well-
///   formed grammars don't use thousands of them).
///
/// This is **not** a full GBNF parser. It catches the common abuse
/// shapes (huge grammar, exponential branching) without the cost of
/// implementing a parser ahead of libllama. Operators who need
/// stricter validation should sanitize at the caller side.
fn validate_grammar(grammar: &str) -> Result<(), LlamaCppError> {
    if grammar.len() > MAX_GRAMMAR_BYTES {
        return Err(LlamaCppError::Sampler);
    }
    let alternations = grammar.bytes().filter(|&b| b == b'|').count();
    if alternations > MAX_GRAMMAR_ALTERNATIONS {
        return Err(LlamaCppError::Sampler);
    }
    Ok(())
}

fn tokenize(
    vocab: *const ffi::llama_vocab,
    text: &[u8],
    add_special: bool,
    parse_special: bool,
) -> Result<Vec<ffi::llama_token>, LlamaCppError> {
    // SAFETY: FFI; first call probes required size.
    let needed = unsafe {
        ffi::llama_tokenize(
            vocab,
            text.as_ptr() as *const std::os::raw::c_char,
            text.len() as i32,
            ptr::null_mut(),
            0,
            add_special,
            parse_special,
        )
    };
    if needed >= 0 {
        // 0 tokens is degenerate but not an error.
        return Ok(vec![0; needed as usize]);
    }
    let need = (-needed) as usize;
    let mut tokens = vec![0i32; need];
    // SAFETY: FFI; buffer sized correctly per the previous probe.
    let written = unsafe {
        ffi::llama_tokenize(
            vocab,
            text.as_ptr() as *const std::os::raw::c_char,
            text.len() as i32,
            tokens.as_mut_ptr(),
            need as i32,
            add_special,
            parse_special,
        )
    };
    if written < 0 {
        return Err(LlamaCppError::Tokenize);
    }
    tokens.truncate(written as usize);
    Ok(tokens)
}

fn token_to_piece(
    vocab: *const ffi::llama_vocab,
    token: ffi::llama_token,
    buf: &mut [u8],
) -> &[u8] {
    // SAFETY: FFI; buffer sized at the call site (256 bytes — always
    // enough for a single token piece in practice; if the value comes
    // back negative we return empty).
    let n = unsafe {
        ffi::llama_token_to_piece(
            vocab,
            token,
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            buf.len() as i32,
            0,
            true,
        )
    };
    if n <= 0 {
        return &[];
    }
    let n = (n as usize).min(buf.len());
    &buf[..n]
}

/// Decode an `Attachment` from the wire shape (raw RGB or f32 PCM,
/// base64-wrapped) into an mtmd `Bitmap`. Per ADR 0016 the daemon
/// does not link image/audio codecs; these payloads are pre-decoded
/// by the consumer.
fn build_bitmap(att: &Attachment) -> Result<Bitmap, LlamaCppError> {
    use base64::engine::general_purpose::STANDARD;
    match att {
        Attachment::Image {
            id,
            width,
            height,
            bytes,
        } => {
            let raw = STANDARD
                .decode(bytes)
                .map_err(|_| LlamaCppError::Base64(id.clone()))?;
            let bm = Bitmap::from_image_rgb(*width, *height, &raw)?;
            Ok(bm)
        }
        Attachment::Audio { id, bytes, .. } => {
            let raw = STANDARD
                .decode(bytes)
                .map_err(|_| LlamaCppError::Base64(id.clone()))?;
            // Reinterpret as f32 LE samples.
            if raw.len() % 4 != 0 {
                return Err(LlamaCppError::Render(format!(
                    "audio attachment {id:?}: byte length not a multiple of 4"
                )));
            }
            let n_samples = raw.len() / 4;
            let mut samples = Vec::with_capacity(n_samples);
            for chunk in raw.chunks_exact(4) {
                let arr: [u8; 4] = chunk.try_into().expect("chunks_exact 4 yields 4");
                samples.push(f32::from_le_bytes(arr));
            }
            Ok(Bitmap::from_audio_f32(&samples)?)
        }
        Attachment::Video { id, .. } => Err(LlamaCppError::Render(format!(
            "video attachment {id:?} not supported by the llamacpp adapter"
        ))),
        Attachment::Unknown => Err(LlamaCppError::Render(
            "unknown attachment kind in resolved request".into(),
        )),
    }
}

fn build_sampler_chain_v2(
    _vocab: *const ffi::llama_vocab,
    req: &ResolvedV2,
    seed: u32,
) -> Result<*mut ffi::llama_sampler, LlamaCppError> {
    // _vocab is unused today (v2 has no GBNF grammar — the model
    // self-constrains tool calls structurally). Kept in the signature
    // to mirror build_sampler_chain's shape so a future grammar
    // extension doesn't require a signature break.
    let temperature = req.temperature.unwrap_or(1.0) as f32;
    let top_p = req.top_p.unwrap_or(0.95) as f32;
    let top_k = req.top_k.unwrap_or(64) as i32;

    // SAFETY: FFI sequence.
    let chain = unsafe {
        let params = ffi::llama_sampler_chain_default_params();
        ffi::llama_sampler_chain_init(params)
    };
    if chain.is_null() {
        return Err(LlamaCppError::Sampler);
    }

    unsafe {
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_k(top_k));
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_top_p(top_p, 1));
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_temp(temperature));
        ffi::llama_sampler_chain_add(chain, ffi::llama_sampler_init_dist(seed));
    }
    Ok(chain)
}

/// v2 generation: tokenise the rendered prompt + bitmaps via mtmd,
/// run the helper-driven encode-and-decode loop to fill the KV cache
/// from the prompt + projected attachments, then sample tokens until
/// EOS or `max_tokens`. Streams `TokenEventV2::Text` for each
/// generated piece; emits one `Done` on clean exit.
///
/// Drop-on-cancel: when the receiver disconnects, the next
/// `tx.blocking_send` errors and the loop exits silently. The daemon
/// translates the missing terminal frame into an `error` (mid-stream
/// failure mapping per ADR 0007).
fn run_generation_v2(
    state: &Arc<Mutex<State>>,
    prompt: &str,
    bitmaps: &[Bitmap],
    req: &ResolvedV2,
    max_new: u32,
    seed: u32,
    tx: &mpsc::Sender<TokenEventV2>,
) -> Result<(), LlamaCppError> {
    let guard = state.lock().expect("poisoned llamacpp state mutex");
    let model = guard.model.as_ptr();
    let ctx = guard.ctx.ptr.as_ptr();
    let mtmd = guard.mtmd.as_ref().ok_or(LlamaCppError::NoMmproj)?;

    // SAFETY: FFI; pointers valid for the lock's lifetime.
    let vocab = unsafe { ffi::llama_model_get_vocab(model) };

    // Reset KV cache so each generation starts clean.
    // SAFETY: FFI; ctx valid.
    unsafe {
        let mem = ffi::llama_get_memory(ctx);
        if !mem.is_null() {
            ffi::llama_memory_clear(mem, true);
        }
    }

    // Tokenise prompt + bitmaps via mtmd.
    let bitmap_refs: Vec<&Bitmap> = bitmaps.iter().collect();
    let chunks = mtmd
        .tokenize(prompt, &bitmap_refs)
        .map_err(LlamaCppError::Mtmd)?;

    // Run upstream's helper-driven eval loop (text chunks ->
    // llama_decode; image/audio chunks -> mtmd_encode then decode
    // via the precomputed embeddings). Returns the new n_past so we
    // can resume sampling from the right position.
    // SAFETY: ctx and chunks are wired together — chunks was just
    // produced from `mtmd` against this ctx's parent model.
    let n_past =
        unsafe { mtmd.eval_chunks(ctx, &chunks, 0, 0, 512, true) }.map_err(LlamaCppError::Mtmd)?;

    // Use mtmd's helper to count prompt-side tokens (including
    // projected media tokens) for the usage report.
    let prompt_tokens = unsafe { crate::mtmd_ffi::mtmd_helper_get_n_tokens(chunks.raw()) } as u32;
    drop(chunks);

    // Sampler chain. v2 sampling fields default if absent (see
    // build_sampler_chain_v2).
    let sampler = build_sampler_chain_v2(vocab, req, seed)?;
    let _sampler_guard = SamplerGuard { ptr: sampler };

    let mut completion_tokens: u32 = 0;
    let mut buf = [0u8; 256];
    let mut n_past = n_past;
    let mut parser = ToolCallParser::new();
    let mut emitted_tool_use = false;

    for _ in 0..max_new {
        // Sample.
        // SAFETY: FFI; sampler + ctx valid in scope.
        let next: ffi::llama_token = unsafe { ffi::llama_sampler_sample(sampler, ctx, -1) };

        // SAFETY: FFI; vocab valid.
        let is_eog = unsafe { ffi::llama_vocab_is_eog(vocab, next) };
        if is_eog {
            // Flush any text the parser was holding before emitting Done.
            for ev in parser.finish() {
                if let Some(out_ev) = parser_output_to_event_v2(ev, &mut emitted_tool_use)
                    && tx.blocking_send(out_ev).is_err()
                {
                    return Ok(());
                }
            }
            let stop = if emitted_tool_use {
                StopReasonV2::ToolUse
            } else {
                StopReasonV2::EndTurn
            };
            let _ = tx.blocking_send(TokenEventV2::Done {
                stop_reason: stop,
                usage: UsageV2 {
                    input_tokens: prompt_tokens,
                    output_tokens: completion_tokens,
                },
            });
            return Ok(());
        }

        // SAFETY: FFI; sampler valid.
        unsafe { ffi::llama_sampler_accept(sampler, next) };

        let piece = token_to_piece(vocab, next, &mut buf);
        let text = String::from_utf8_lossy(piece).into_owned();
        // Run through the tool/thinking parser. The parser may emit
        // 0 or more events per piece (text deltas, thinking deltas,
        // complete tool_use, or malformed).
        for ev in parser.push(&text) {
            if let TokenOutput::Malformed(reason) = &ev {
                warn!(reason = %reason, "tool-call parse failed; aborting generation");
                // Mid-stream malformed -> terminate stream silently;
                // daemon translates to BackendUnavailable. (We could
                // add a ToolCallMalformed code path through
                // GenerateError but that's a larger refactor.)
                return Err(LlamaCppError::Render(reason.clone()));
            }
            if let Some(out_ev) = parser_output_to_event_v2(ev, &mut emitted_tool_use)
                && tx.blocking_send(out_ev).is_err()
            {
                debug!("v2 generation cancelled (receiver dropped)");
                return Ok(());
            }
        }
        completion_tokens = completion_tokens.saturating_add(1);

        // Feed the new token back. n_past advances by 1 per token.
        let mut next_arr = [next];
        // SAFETY: FFI; next_arr lives for the call.
        let batch = unsafe { ffi::llama_batch_get_one(next_arr.as_mut_ptr(), 1) };
        let rc = unsafe { ffi::llama_decode(ctx, batch) };
        if rc != 0 {
            return Err(LlamaCppError::Decode(rc));
        }
        n_past = n_past.saturating_add(1);
    }

    // max_tokens reached. Flush any remaining parser state.
    for ev in parser.finish() {
        if let Some(out_ev) = parser_output_to_event_v2(ev, &mut emitted_tool_use)
            && tx.blocking_send(out_ev).is_err()
        {
            return Ok(());
        }
    }
    let _ = tx.blocking_send(TokenEventV2::Done {
        stop_reason: StopReasonV2::MaxTokens,
        usage: UsageV2 {
            input_tokens: prompt_tokens,
            output_tokens: completion_tokens,
        },
    });
    Ok(())
}

/// Map a `ToolCallParser::Output` to a `TokenEventV2`. Sets
/// `emitted_tool_use` when a `ToolUse` is emitted so the terminal
/// stop_reason can be set correctly. Returns `None` for the
/// `Malformed` variant (the caller handles that path separately
/// via an early return).
fn parser_output_to_event_v2(ev: TokenOutput, emitted_tool_use: &mut bool) -> Option<TokenEventV2> {
    match ev {
        TokenOutput::Text(text) => {
            if text.is_empty() {
                None
            } else {
                Some(TokenEventV2::Text(text))
            }
        }
        TokenOutput::Thinking(text) => {
            if text.is_empty() {
                None
            } else {
                Some(TokenEventV2::Thinking(text))
            }
        }
        TokenOutput::ToolUse {
            tool_call_id,
            name,
            input,
        } => {
            *emitted_tool_use = true;
            Some(TokenEventV2::ToolUse {
                tool_call_id,
                name,
                input,
            })
        }
        TokenOutput::Malformed(_) => None,
    }
}

#[cfg(test)]
mod grammar_tests {
    use super::*;

    #[test]
    fn small_grammar_is_accepted() {
        let g = r#"root ::= "yes" | "no""#;
        validate_grammar(g).unwrap();
    }

    #[test]
    fn realistic_json_grammar_is_accepted() {
        // ~700 bytes; well below MAX_GRAMMAR_BYTES.
        let g = r#"
            root   ::= object
            object ::= "{" ws members? ws "}"
            members ::= pair ("," ws pair)*
            pair   ::= string ws ":" ws value
            value  ::= object | string | number | "true" | "false" | "null"
            string ::= "\"" [^"]* "\""
            number ::= [0-9]+ ("." [0-9]+)?
            ws     ::= [ \t\n]*
        "#;
        validate_grammar(g).unwrap();
    }

    #[test]
    fn oversized_grammar_is_rejected() {
        let g = "x".repeat(MAX_GRAMMAR_BYTES + 1);
        assert!(validate_grammar(&g).is_err());
    }

    #[test]
    fn excessive_alternations_rejected() {
        let g = "|".repeat(MAX_GRAMMAR_ALTERNATIONS + 1);
        assert!(validate_grammar(&g).is_err());
    }

    #[test]
    fn alternation_count_under_threshold_accepted() {
        let g = "|".repeat(MAX_GRAMMAR_ALTERNATIONS);
        validate_grammar(&g).unwrap();
    }
}
