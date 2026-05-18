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

use crate::backend::{Backend, GenerateError, TokenEvent, TokenStream};
use crate::ffi;
use crate::llamacpp::loader::{load_model, ModelHandle, ModelLoadError};
use async_trait::async_trait;
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
}

impl Default for LlamaCppConfig {
    fn default() -> Self {
        Self {
            model_path: std::path::PathBuf::new(),
            model_sha256: None,
            n_ctx: 8192,
            n_gpu_layers: 0,
            seed: 0xDEADBEEF,
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
    /// Shared so the spawn_blocking generation task can reach the model
    /// and context. Locked for the duration of one generation; the
    /// daemon's queue serialises calls, so contention is structural
    /// (always 1 holder + 0 waiters in v0.1).
    state: Arc<Mutex<State>>,
}

struct State {
    model: ModelHandle,
    ctx: ContextHandle,
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

        Ok(Self {
            name: "llamacpp",
            ready: AtomicBool::new(true),
            seed: config.seed,
            state: Arc::new(Mutex::new(State { model, ctx })),
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

/// Apply the model's chat template to the messages array and return the
/// rendered prompt as bytes. Returns `None` if the model has no template
/// or the call fails.
fn render_chat_template(
    state: &Arc<Mutex<State>>,
    messages: &[inferd_proto::Message],
) -> Option<Vec<u8>> {
    let guard = state.lock().ok()?;

    // Build the C-side `llama_chat_message` array.
    let mut roles: Vec<CString> = Vec::with_capacity(messages.len());
    let mut contents: Vec<CString> = Vec::with_capacity(messages.len());
    for m in messages {
        roles.push(CString::new(role_str(m.role)).ok()?);
        contents.push(CString::new(m.content.as_str()).ok()?);
    }

    let chat: Vec<ffi::llama_chat_message> = roles
        .iter()
        .zip(contents.iter())
        .map(|(r, c)| ffi::llama_chat_message {
            role: r.as_ptr(),
            content: c.as_ptr(),
        })
        .collect();

    // SAFETY: pointers in `chat` outlive the call (roles/contents kept
    // alive in this scope). `model_chat_template` returns a borrowed
    // C string owned by libllama; we copy out before letting it drop.
    let tmpl_ptr = unsafe { ffi::llama_model_chat_template(guard.model.as_ptr(), ptr::null()) };

    let mut out = vec![0u8; messages.iter().map(|m| m.content.len()).sum::<usize>() * 2 + 1024];
    loop {
        // SAFETY: FFI. `chat.as_ptr()` valid for `chat.len()` entries;
        // `out.as_mut_ptr()` valid for `out.len()` bytes.
        let n = unsafe {
            ffi::llama_chat_apply_template(
                tmpl_ptr,
                chat.as_ptr(),
                chat.len(),
                true,
                out.as_mut_ptr() as *mut std::os::raw::c_char,
                out.len() as i32,
            )
        };
        if n < 0 {
            return None;
        }
        if (n as usize) <= out.len() {
            out.truncate(n as usize);
            return Some(out);
        }
        // Need more space; resize and retry.
        out.resize(n as usize, 0);
    }
}

fn role_str(r: inferd_proto::Role) -> &'static str {
    match r {
        inferd_proto::Role::System => "system",
        inferd_proto::Role::User => "user",
        inferd_proto::Role::Assistant => "assistant",
    }
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
