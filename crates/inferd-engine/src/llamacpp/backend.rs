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
    AcceleratorInfo, AcceleratorKind, Backend, BackendCapabilities, EmbedError, EmbedResult,
    GenerateError, TokenEventV2, TokenStreamV2,
};
use crate::ffi;
use crate::llamacpp::chat_template::Gemma4Renderer;
use crate::llamacpp::grammar;
use crate::llamacpp::loader::{ModelHandle, ModelLoadError, load_model};
use crate::llamacpp::mtmd::{Bitmap, Mtmd, MtmdConfig, MtmdError};
use crate::llamacpp::tool_parser::{Output as TokenOutput, ToolCallParser};
use async_trait::async_trait;
use inferd_proto::embed::{EmbedResolved, EmbedUsage};
use inferd_proto::v2::{Attachment, ResolvedV2, StopReasonV2, UsageV2};
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
    /// Maximum image tokens per image for dynamic-resolution vision
    /// models (Gemma 4). A higher cap means less downscaling, so small
    /// or sparsely-spaced text (OCR of fine print, leader-dotted lines)
    /// survives — at the cost of more tokens and slower encode. `None`
    /// reads the model metadata default. Sets
    /// `MtmdConfig::image_max_tokens` at mtmd init (issue #42).
    pub mmproj_image_max_tokens: Option<i32>,
    /// Enable the embedding pathway (per ADR 0017). When `true` the
    /// adapter allocates a second `llama_context` configured with
    /// `embeddings = true` + `pooling_type = MEAN`, advertises
    /// `BackendCapabilities::embed = true`, and serves
    /// `Backend::embed`. Defaults to `false` so generation-only
    /// deployments don't pay the second-context allocation cost.
    pub embed: bool,
    /// Pooling type for the embed context. `LLAMA_POOLING_TYPE_MEAN`
    /// (1) is the EmbeddingGemma default and is the value used when
    /// this is `None`. Other values map directly to the libllama
    /// `llama_pooling_type` enum.
    pub embed_pooling: Option<i32>,
    /// Context window for the embed context. EmbeddingGemma 300M
    /// supports up to 2048; defaults to that. Larger inputs produce
    /// `EmbedError::InvalidRequest`.
    pub embed_n_ctx: u32,
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
            mmproj_image_max_tokens: None,
            embed: false,
            embed_pooling: None,
            embed_n_ctx: 2048,
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
    /// Model identifier reported on `done` / `embeddings` frames. Read
    /// from GGUF `general.name` metadata when the model exposes it,
    /// otherwise derived from the file stem (e.g.
    /// `embeddinggemma-300m-Q8_0` from `embeddinggemma-300m-Q8_0.gguf`).
    /// Stable for the lifetime of the adapter; cached so we don't pay
    /// an FFI roundtrip per request.
    model_label: String,
    /// Shared so the spawn_blocking generation task can reach the model
    /// and context. Locked for the duration of one generation; the
    /// daemon's queue serialises calls, so contention is structural
    /// (always 1 holder + 0 waiters in v0.1).
    state: Arc<Mutex<State>>,
}

/// Pick the active GGML accelerator.
///
/// With `dl-backends` on (v0.3 / ADR 0019): runtime probe of the ggml
/// backend registry. `ggml_backend_load_all` dlopens every MODULE
/// shipped next to the daemon binary; the probe walks the registered
/// list and picks per the cascade Metal > CUDA > ROCm > Vulkan > CPU.
/// Operators can force a specific backend via the
/// `INFERD_FORCE_BACKEND` env var. Result is cached process-wide.
///
/// Without `dl-backends` (v0.2.x compatibility path): static-build
/// shape, single accelerator picked at compile time per the
/// `cuda` / `metal` / `vulkan` / `rocm` cargo features.
fn pick_accelerator_kind() -> AcceleratorKind {
    #[cfg(feature = "dl-backends")]
    {
        super::accelerator::probe_accelerator()
    }
    #[cfg(not(feature = "dl-backends"))]
    {
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
}

/// Build the `AcceleratorInfo` that the adapter caches and exposes
/// through `capabilities()`.
///
/// On `dl-backends` builds, walks the ggml device list to find the
/// device matching `kind` and reads its name + VRAM total. The
/// CPU branch and the no-match branch both yield `device_name = None,
/// vram_total_bytes = None`. On the static-build path the device API
/// is not exercised (the registry only has the one compile-pinned
/// backend), so the fields stay `None`.
fn build_accelerator_info(kind: AcceleratorKind, n_gpu_layers: i32) -> AcceleratorInfo {
    let gpu_layers = n_gpu_layers.max(0) as u32;
    #[cfg(feature = "dl-backends")]
    {
        let details = super::accelerator::probe_device_for_kind(kind);
        AcceleratorInfo {
            kind,
            gpu_layers,
            device_name: details.name,
            vram_total_bytes: details.total_bytes,
        }
    }
    #[cfg(not(feature = "dl-backends"))]
    {
        AcceleratorInfo {
            kind,
            gpu_layers,
            device_name: None,
            vram_total_bytes: None,
        }
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
    /// Dedicated embedding context. `Some` when the adapter was
    /// configured with `embed = true`. Allocated alongside the
    /// generation context so embed and generate calls don't fight
    /// over `llama_set_embeddings`. Drop order (embed → ctx → model)
    /// ensures the embed context is freed before the parent model.
    embed: Option<EmbedContext>,
}

/// Owned `llama_context` reserved for embedding. Same shape as
/// `ContextHandle` but kept as its own type so a future divergence in
/// per-context state (e.g. cached batch) doesn't need a rename.
struct EmbedContext {
    ctx: ContextHandle,
    /// Embedding dimension reported by `llama_n_embd` at construction
    /// time. Cached so `embed()` can size the output vectors without
    /// re-querying.
    n_embd: u32,
    /// Physical batch size (`n_ubatch`) of this context. Cached so
    /// `run_embed` can reject oversized inputs *before* calling
    /// `llama_encode` — libllama asserts `n_ubatch >= n_tokens` inside
    /// the encoder and aborts the whole process, so the structured
    /// rejection is the only path to keep the daemon alive when a
    /// caller sends a too-long input (issue #20).
    n_ubatch: u32,
}

/// Internal capability snapshot used by `Backend::capabilities()`.
#[derive(Debug, Clone, Copy)]
struct BackendCapabilitiesV2 {
    vision: bool,
    audio: bool,
    /// Sample rate the mmproj's audio encoder expects, in Hz. Read once
    /// at mtmd init (the value is fixed for the loaded mmproj) and
    /// cached so both `capabilities()` and the per-attachment rate check
    /// can read it without an FFI call per request. `None` on a
    /// vision-only mmproj.
    audio_sample_rate: Option<u32>,
}

impl LlamaCpp {
    /// Build a new `LlamaCpp` adapter. Performs model load + context
    /// allocation synchronously. `Backend::ready()` returns `true` once
    /// this returns `Ok`.
    pub fn new(config: LlamaCppConfig) -> Result<Self, LlamaCppError> {
        ensure_backend_init();

        // ADR 0019: with `dl-backends`, the ggml backend registry is
        // empty until ggml_backend_load_all() dlopens the MODULE libs
        // shipped next to the daemon. Run the probe *before*
        // load_model so the model loader sees the registered
        // accelerators when it decides how to honour `n_gpu_layers`.
        // probe_accelerator() is cached, so subsequent adapter
        // constructions reuse the first probe's result. Compile-time
        // path is a no-op aside from the constant-folded match below.
        let kind = pick_accelerator_kind();

        // Gate GPU offload on the chosen accelerator. When the probe (or
        // an `INFERD_FORCE_BACKEND=cpu` override) selects CPU, force
        // `n_gpu_layers = 0` regardless of the configured value. Without
        // this, a GPU host whose operator forced CPU still offloads to the
        // registered GPU device — llama.cpp only clamps the configured
        // count to 0 when *no* GPU device is present, not when the operator
        // asked for CPU on a GPU box. This keeps the ADR 0019 escape hatch
        // honest: forcing CPU actually runs on CPU. Any non-CPU kind passes
        // the configured value through unchanged (`-1` = offload all).
        let effective_gpu_layers = if kind == AcceleratorKind::Cpu {
            0
        } else {
            config.n_gpu_layers
        };

        let model = load_model(
            &config.model_path,
            config.model_sha256.as_ref(),
            effective_gpu_layers,
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
                let mtmd_config = MtmdConfig {
                    image_max_tokens: config.mmproj_image_max_tokens,
                    ..MtmdConfig::default()
                };
                let mtmd_ctx = unsafe { Mtmd::new(mmproj, model.as_ptr(), mtmd_config)? };
                let audio = mtmd_ctx.supports_audio();
                let caps = BackendCapabilitiesV2 {
                    vision: mtmd_ctx.supports_vision(),
                    audio,
                    // Only meaningful when the mmproj actually has an
                    // audio projector; mtmd reports 0 (→ None) otherwise.
                    audio_sample_rate: if audio {
                        mtmd_ctx.audio_sample_rate()
                    } else {
                        None
                    },
                };
                (Some(mtmd_ctx), Some(caps))
            }
            None => (None, None),
        };

        let accelerator = build_accelerator_info(kind, effective_gpu_layers);

        // Resolve a stable, human-meaningful model label. Try GGUF
        // `general.name` metadata first; fall back to the file stem.
        // Diagnostic-only per ADR 0007 — apps must not branch on it —
        // but still must be accurate (saying "llamacpp" when the
        // backend's `name()` already exposes that is wrong twice).
        let model_label = read_model_label(model.as_ptr(), &config.model_path);

        // Optional dedicated embedding context. Built with
        // `embeddings = true` + a configurable pooling_type (default
        // MEAN, what EmbeddingGemma expects). Kept alongside the
        // generation context so `Backend::embed` doesn't toggle
        // `llama_set_embeddings` on the generation context — that
        // would corrupt active generations on the same context.
        let embed = if config.embed {
            // SAFETY: FFI. `model.as_ptr()` is non-null and valid.
            // `params` is POD initialised by libllama.
            let embed_ctx_ptr = unsafe {
                let mut params = ffi::llama_context_default_params();
                params.n_ctx = config.embed_n_ctx;
                // libllama's encoder asserts `n_ubatch >= n_tokens`. The
                // default `n_ubatch` is 512, so a single input >512
                // tokens (~2KB English) fires GGML_ASSERT and aborts the
                // whole daemon (issue #20). Size the logical and
                // physical batch to the full context window so any
                // input that fits in n_ctx also fits in one ubatch.
                params.n_batch = config.embed_n_ctx;
                params.n_ubatch = config.embed_n_ctx;
                params.embeddings = true;
                params.pooling_type = config.embed_pooling.unwrap_or(ffi::LLAMA_POOLING_TYPE_MEAN);
                ffi::llama_init_from_model(model.as_ptr(), params)
            };
            let embed_ctx = NonNull::new(embed_ctx_ptr)
                .map(|ptr| ContextHandle { ptr })
                .ok_or(LlamaCppError::ContextInit)?;
            // SAFETY: FFI; `model.as_ptr()` valid.
            let n_embd = unsafe { ffi::llama_n_embd(model.as_ptr()) };
            if n_embd <= 0 {
                return Err(LlamaCppError::ContextInit);
            }
            // SAFETY: FFI; we just allocated embed_ctx and hold it
            // exclusively. libllama may clamp `n_ubatch` to `n_batch`
            // or to model limits, so query the actual value rather
            // than trusting the requested params.
            let n_ubatch = unsafe { ffi::llama_n_ubatch(embed_ctx.ptr.as_ptr()) };
            Some(EmbedContext {
                ctx: embed_ctx,
                n_embd: n_embd as u32,
                n_ubatch,
            })
        } else {
            None
        };

        Ok(Self {
            name: "llamacpp",
            ready: AtomicBool::new(true),
            seed: config.seed,
            accelerator,
            model_label,
            state: Arc::new(Mutex::new(State {
                model,
                ctx,
                mtmd,
                caps_v2,
                embed,
            })),
        })
    }
}

/// Read a stable model identifier for diagnostic frames.
///
/// Order:
/// 1. GGUF `general.name` metadata (the canonical model name as
///    encoded by the producer — e.g. `"EmbeddingGemma 300M"` or
///    `"Gemma-4-9B-Instruct"`).
/// 2. Path file stem (e.g. `embeddinggemma-300m-Q8_0.gguf` →
///    `embeddinggemma-300m-Q8_0`).
/// 3. Constant `"llamacpp"` as a last resort if the path has no
///    valid Unicode stem (extremely unusual).
fn read_model_label(model: *const ffi::llama_model, path: &std::path::Path) -> String {
    if let Some(name) = read_gguf_meta_string(model, "general.name") {
        return name;
    }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        return stem.to_string();
    }
    "llamacpp".to_string()
}

/// Look up a string-valued GGUF metadata key on a loaded model.
///
/// Returns `None` if the key is absent, the value is empty, or the
/// FFI surface returns a negative length. Allocates a 256-byte stack
/// buffer first, retries with a heap buffer sized to the FFI's
/// reported length if 256 bytes is too small (cheap insurance — most
/// metadata strings are far under 64 bytes).
fn read_gguf_meta_string(model: *const ffi::llama_model, key: &str) -> Option<String> {
    let key_c = CString::new(key).ok()?;
    // First pass: stack buffer.
    let mut buf = [0i8; 256];
    // SAFETY: FFI; `model` valid, `key_c` lives for the call,
    // `buf` covers `buf.len()` bytes.
    let needed = unsafe {
        ffi::llama_model_meta_val_str(
            model,
            key_c.as_ptr(),
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            buf.len(),
        )
    };
    if needed < 0 {
        return None;
    }
    let needed = needed as usize;
    if needed == 0 {
        return None;
    }
    if needed < buf.len() {
        // SAFETY: FFI wrote `needed` bytes + NUL into `buf`.
        let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const _) };
        return cstr.to_str().ok().map(|s| s.to_string());
    }
    // Stack buffer too small — retry with a heap buffer of `needed + 1`.
    let mut heap = vec![0i8; needed + 1];
    // SAFETY: FFI; same contract as above.
    let n = unsafe {
        ffi::llama_model_meta_val_str(
            model,
            key_c.as_ptr(),
            heap.as_mut_ptr() as *mut std::os::raw::c_char,
            heap.len(),
        )
    };
    if n < 0 {
        return None;
    }
    // SAFETY: FFI wrote up to `n` bytes + NUL into `heap`.
    let cstr = unsafe { std::ffi::CStr::from_ptr(heap.as_ptr() as *const _) };
    cstr.to_str().ok().map(|s| s.to_string())
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
        // Read the cached caps probed at construction. As of v0.4
        // (ADR 0021) v2 is the *only* generation surface — a text-only
        // request is a single text content block — so any llamacpp
        // generation backend advertises `v2: true`. The mmproj probe
        // (`caps_v2`) only governs the multimodal sub-capabilities
        // (`vision` / `audio`): without an mmproj we still generate
        // text, we just reject image / audio attachments.
        //
        // (Before v0.4, text-only generation rode the separate v1
        // surface, so `v2` could be false here without breaking
        // generation. Folding v1 into v2 made `v2: true` mandatory for
        // every generation-capable backend — otherwise the daemon's
        // v2-capability gate refuses every real request. Regression
        // caught by tests/echo_llamacpp.rs.)
        let (snap, embed) = {
            let guard = self.state.lock().expect("poisoned llamacpp state mutex");
            (guard.caps_v2, guard.embed.is_some())
        };
        BackendCapabilities {
            v2: true,
            vision: snap.map(|c| c.vision).unwrap_or(false),
            audio: snap.map(|c| c.audio).unwrap_or(false),
            audio_sample_rate: snap.and_then(|c| c.audio_sample_rate),
            video: false,
            tools: true,
            thinking: true,
            embed,
            accelerator: self.accelerator.clone(),
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

        // Rate the loaded mmproj's audio encoder requires, for the
        // per-attachment check in `build_bitmap`. Read once here rather
        // than per attachment so we take the state lock a single time.
        let expected_audio_rate = {
            let guard = self.state.lock().expect("poisoned llamacpp state mutex");
            guard.caps_v2.and_then(|c| c.audio_sample_rate)
        };

        // Decode each referenced attachment's bytes into Bitmaps.
        let bitmaps: Vec<Bitmap> = rendered
            .attachments
            .iter()
            .map(|att| build_bitmap(att, expected_audio_rate))
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

    async fn embed(&self, req: EmbedResolved) -> Result<EmbedResult, EmbedError> {
        if !self.ready() {
            return Err(EmbedError::NotReady);
        }

        // Pre-stamp inputs with the EmbeddingGemma task prefix on the
        // calling task so the spawn_blocking closure sees the final
        // text. Synchronous + cheap.
        let task = req.task.clone();
        let prefixed: Vec<String> = req
            .input
            .iter()
            .map(|s| apply_task_prefix(task.as_ref(), s))
            .collect();
        let dimensions = req.dimensions;
        let label = self.model_label.clone();

        let state = Arc::clone(&self.state);
        // FFI must run on a blocking thread so it doesn't stall the
        // tokio runtime.
        tokio::task::spawn_blocking(move || run_embed(&state, &prefixed, dimensions, label))
            .await
            .map_err(|e| EmbedError::Internal(format!("embed task join: {e}")))?
    }

    async fn stop(&self, _timeout: Duration) -> Result<(), GenerateError> {
        // Mark not-ready so any in-flight `generate` calls error before
        // touching the FFI. Drop will free model + context when the
        // adapter itself is dropped.
        self.ready.store(false, Ordering::SeqCst);
        Ok(())
    }
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

/// Reject an audio attachment whose declared sample rate isn't the one
/// the model's audio encoder requires.
///
/// mtmd's audio entry point takes a bare `&[f32]` with no rate argument,
/// so the encoder reads whatever it is given at its *own* rate. 44.1 kHz
/// PCM fed to a 16 kHz encoder is therefore not detectable from the
/// samples — it just decodes ~2.75× too fast and yields a
/// plausible-looking wrong answer. The daemon does not resample (that is
/// the consumer's job under ADR 0016), so a loud rejection naming both
/// rates is the only honest option.
///
/// Split out of [`build_bitmap`] so it is testable without an mmproj on
/// disk — `Bitmap` construction needs live FFI, this decision doesn't.
///
/// `expected` of `None` means the loaded mmproj reported no rate, so
/// there is nothing to compare against and the attachment passes: an
/// invented constraint would reject valid requests.
fn check_audio_sample_rate(
    id: &str,
    declared: u32,
    expected: Option<u32>,
) -> Result<(), LlamaCppError> {
    match expected {
        Some(expected) if declared != expected => Err(LlamaCppError::Render(format!(
            "audio attachment {id:?}: sample_rate {declared} Hz does not match the model's \
             audio encoder, which requires {expected} Hz; resample before sending (the \
             daemon does not resample)"
        ))),
        _ => Ok(()),
    }
}

/// Turn an `Attachment` into an mtmd `Bitmap`. As of ADR 0021 the
/// attachment's `bytes` are the **raw** decoded payload (interleaved RGB
/// for images, little-endian f32 PCM for audio) delivered out-of-band in
/// a BLOB frame — no base64. Per ADR 0016 the daemon links no
/// image/audio codec; the consumer pre-decodes.
///
/// `expected_audio_rate` is the loaded mmproj's required sample rate (see
/// [`BackendCapabilitiesV2::audio_sample_rate`]); audio attachments are
/// checked against it by [`check_audio_sample_rate`].
fn build_bitmap(
    att: &Attachment,
    expected_audio_rate: Option<u32>,
) -> Result<Bitmap, LlamaCppError> {
    match att {
        Attachment::Image {
            width,
            height,
            bytes,
            ..
        } => {
            let bm = Bitmap::from_image_rgb(*width, *height, bytes)?;
            Ok(bm)
        }
        Attachment::Audio {
            id,
            bytes,
            sample_rate,
            ..
        } => {
            check_audio_sample_rate(id, *sample_rate, expected_audio_rate)?;
            // Reinterpret raw bytes as f32 LE samples.
            if bytes.len() % 4 != 0 {
                return Err(LlamaCppError::Render(format!(
                    "audio attachment {id:?}: byte length not a multiple of 4"
                )));
            }
            let n_samples = bytes.len() / 4;
            let mut samples = Vec::with_capacity(n_samples);
            for chunk in bytes.chunks_exact(4) {
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
    // The grammar sampler is NOT added here. Per llama.cpp's reference
    // `common_sampler` (common/sampling.cpp), a grammar sampler must be
    // kept SEPARATE from the chain and applied out-of-band per token
    // (apply-grammar → apply-chain → accept) — driving it through
    // `llama_sampler_sample` on the chain throws a C++ exception that
    // aborts the process across FFI. See `build_grammar_sampler_v2` +
    // the grammar handling in `run_generation_v2`.
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

/// Build the standalone grammar sampler for a `response_format`
/// constraint, or `None` when the request carries no constraint.
///
/// Kept separate from the sampler chain on purpose (ADR 0013 + the
/// llama.cpp `common_sampler` pattern): the grammar sampler is applied
/// to the candidate logits and `accept`-ed manually in the decode loop,
/// never sampled via `llama_sampler_sample`. A bad schema returns an
/// error here — it must never reach a throw-across-FFI that would abort
/// the daemon.
fn build_grammar_sampler_v2(
    vocab: *const ffi::llama_vocab,
    req: &ResolvedV2,
) -> Result<Option<*mut ffi::llama_sampler>, LlamaCppError> {
    let Some(format) = &req.response_format else {
        return Ok(None);
    };
    let inferd_proto::v2::ResponseFormat::JsonSchema { schema } = format;

    // Compile JSON Schema → GBNF (the shim catches C++ exceptions and
    // returns an error rather than unwinding into Rust).
    let gbnf = grammar::json_schema_to_gbnf(schema).map_err(|_| LlamaCppError::Sampler)?;
    let gbnf_cstr = CString::new(gbnf).map_err(|_| LlamaCppError::Sampler)?;
    let root = CString::new("root").map_err(|_| LlamaCppError::Sampler)?;

    // SAFETY: FFI; vocab valid for the lock's lifetime. init_grammar
    // returns NULL (never throws) on a malformed grammar.
    let grmr = unsafe { ffi::llama_sampler_init_grammar(vocab, gbnf_cstr.as_ptr(), root.as_ptr()) };
    if grmr.is_null() {
        return Err(LlamaCppError::Sampler);
    }
    Ok(Some(grmr))
}

/// Sample one token under a grammar constraint, following llama.cpp's
/// `common_sampler_sample` grammar-first path: build the candidate array
/// from the last logits, apply the grammar sampler (masks
/// non-conforming tokens to -inf), then apply the chain
/// (top_k/top_p/temp/dist) which selects from the survivors. Returns the
/// selected token id.
///
/// SAFETY: `ctx` must have just decoded at least one token (logits for
/// index -1 valid); `chain` and `grmr` must be live samplers built for
/// this context's vocab; `n_vocab` must equal the model vocab size.
/// `cur` is the caller-owned candidate buffer, reused across tokens
/// (upstream's `common_sampler` keeps the same buffer alive for the same
/// reason); it is refilled here and its prior contents are irrelevant.
unsafe fn sample_with_grammar(
    ctx: *mut ffi::llama_context,
    chain: *mut ffi::llama_sampler,
    grmr: *mut ffi::llama_sampler,
    cur: &mut Vec<ffi::llama_token_data>,
    n_vocab: usize,
) -> ffi::llama_token {
    // Logits for the most recent token (-1).
    let logits = unsafe { ffi::llama_get_logits_ith(ctx, -1) };
    cur.clear();
    cur.extend((0..n_vocab).map(|i| ffi::llama_token_data {
        id: i as ffi::llama_token,
        // SAFETY: logits points to at least n_vocab floats.
        logit: unsafe { *logits.add(i) },
        p: 0.0,
    }));

    let mut cur_p = ffi::llama_token_data_array {
        data: cur.as_mut_ptr(),
        size: cur.len(),
        selected: -1,
        sorted: false,
    };

    // Grammar first: mask non-conforming tokens. Then the chain selects.
    unsafe {
        ffi::llama_sampler_apply(grmr, &mut cur_p);
        ffi::llama_sampler_apply(chain, &mut cur_p);
    }

    // The chain's dist sampler set `selected`; fall back to argmax-safe 0.
    let sel = cur_p.selected;
    if sel < 0 || (sel as usize) >= cur.len() {
        // Defensive: no selection (shouldn't happen with a dist sampler) —
        // return the first candidate id rather than indexing OOB.
        return cur.first().map(|d| d.id).unwrap_or(0);
    }
    cur[sel as usize].id
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

    // Prompt prefill. Two paths (ADR 0021 — v2 is the single generation
    // surface, so this must handle the text-only case that v1 used to):
    //   - mtmd present: tokenise prompt + bitmaps through libmtmd and run
    //     its helper eval loop (text chunks → llama_decode, media chunks
    //     → mtmd_encode then decode). Required when there are bitmaps.
    //   - no mtmd (text-only model, no mmproj): plain tokenise +
    //     llama_decode. A text-only model has no mtmd context, and
    //     rejecting generation here was the v1→v2 fold regression that
    //     broke every text-only request (tests/echo_llamacpp.rs).
    // A request that carries bitmaps but the model has no mtmd is a
    // caller error — attachments can't be projected without an mmproj.
    let (n_past, prompt_tokens) = match guard.mtmd.as_ref() {
        Some(mtmd) => {
            let bitmap_refs: Vec<&Bitmap> = bitmaps.iter().collect();
            let chunks = mtmd
                .tokenize(prompt, &bitmap_refs)
                .map_err(LlamaCppError::Mtmd)?;
            // SAFETY: ctx and chunks are wired together — chunks was just
            // produced from `mtmd` against this ctx's parent model.
            let n_past = unsafe { mtmd.eval_chunks(ctx, &chunks, 0, 0, 512, true) }
                .map_err(LlamaCppError::Mtmd)?;
            // mtmd's token count includes projected media tokens.
            let prompt_tokens =
                unsafe { crate::mtmd_ffi::mtmd_helper_get_n_tokens(chunks.raw()) } as u32;
            drop(chunks);
            (n_past, prompt_tokens)
        }
        None => {
            if !bitmaps.is_empty() {
                return Err(LlamaCppError::NoMmproj);
            }
            // Text-only prefill: tokenise + single batch decode.
            let mut tokens = tokenize(vocab, prompt.as_bytes(), true, true)?;
            let prompt_tokens = tokens.len() as u32;
            // SAFETY: FFI; tokens.as_mut_ptr() valid for tokens.len().
            let batch =
                unsafe { ffi::llama_batch_get_one(tokens.as_mut_ptr(), tokens.len() as i32) };
            let rc = unsafe { ffi::llama_decode(ctx, batch) };
            if rc != 0 {
                return Err(LlamaCppError::Decode(rc));
            }
            (prompt_tokens as i32, prompt_tokens)
        }
    };

    // Sampler chain. v2 sampling fields default if absent (see
    // build_sampler_chain_v2).
    let sampler = build_sampler_chain_v2(vocab, req, seed)?;
    let _sampler_guard = SamplerGuard { ptr: sampler };

    // Optional standalone grammar sampler (response_format → GBNF). Kept
    // OUT of the chain and applied manually per token, mirroring
    // llama.cpp's `common_sampler` — chaining it would throw across FFI.
    let grammar_sampler = build_grammar_sampler_v2(vocab, req)?;
    let _grammar_guard = grammar_sampler.map(|ptr| SamplerGuard { ptr });
    let n_vocab = unsafe { ffi::llama_vocab_n_tokens(vocab) } as usize;
    // Candidate buffer for the grammar path, allocated once and reused for
    // every token — it is vocab-sized (~262k × 12 B ≈ 3 MiB for Gemma 4),
    // so allocating per token would churn gigabytes over a long completion.
    // Empty (no allocation) when there is no grammar constraint.
    let mut candidates: Vec<ffi::llama_token_data> = Vec::with_capacity(match grammar_sampler {
        Some(_) => n_vocab,
        None => 0,
    });

    let mut completion_tokens: u32 = 0;
    let mut buf = [0u8; 256];
    let mut n_past = n_past;
    let mut parser = ToolCallParser::new();
    let mut emitted_tool_use = false;

    for _ in 0..max_new {
        // Sample. With a grammar constraint we run the two-phase
        // grammar-first path (apply grammar to candidates → apply chain →
        // accept); otherwise the chain samples directly.
        //
        // The two paths differ in who accepts: `llama_sampler_sample`
        // accepts into the chain itself (llama-sampler.cpp:869), whereas
        // the grammar path only *applies* the chain, so we must accept
        // explicitly. `chain_needs_accept` tracks that difference — a
        // second accept on an already-accepted chain would double-advance
        // any stateful member (penalties, dry, grammar).
        // SAFETY: FFI; sampler + ctx valid in scope.
        let (next, chain_needs_accept): (ffi::llama_token, bool) = match grammar_sampler {
            Some(grmr) => (
                unsafe { sample_with_grammar(ctx, sampler, grmr, &mut candidates, n_vocab) },
                true,
            ),
            None => (
                unsafe { ffi::llama_sampler_sample(sampler, ctx, -1) },
                false,
            ),
        };

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

        if chain_needs_accept {
            // SAFETY: FFI; sampler valid.
            unsafe { ffi::llama_sampler_accept(sampler, next) };
        }
        // Advance the grammar state with the accepted token, if any.
        if let Some(grmr) = grammar_sampler {
            // SAFETY: FFI; grammar sampler valid for the loop's lifetime.
            unsafe { ffi::llama_sampler_accept(grmr, next) };
        }

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

/// Apply the EmbeddingGemma task-prefix convention. The prefixes are
/// the documented strings the model was trained with; backends that
/// don't apply prefixes ignore the field. `None` returns the input
/// unchanged.
fn apply_task_prefix(task: Option<&inferd_proto::embed::EmbedTask>, input: &str) -> String {
    use inferd_proto::embed::EmbedTask;
    let prefix = match task {
        None | Some(EmbedTask::Other) => return input.to_string(),
        Some(EmbedTask::RetrievalQuery) => "task: search result | query: ",
        Some(EmbedTask::RetrievalDocument) => "title: none | text: ",
        Some(EmbedTask::Similarity) => "task: sentence similarity | query: ",
        Some(EmbedTask::Classification) => "task: classification | query: ",
        Some(EmbedTask::Clustering) => "task: clustering | query: ",
        Some(EmbedTask::QuestionAnswering) => "task: question answering | query: ",
        Some(EmbedTask::FactVerification) => "task: fact checking | query: ",
        Some(EmbedTask::CodeRetrievalQuery) => "task: code retrieval | query: ",
    };
    let mut out = String::with_capacity(prefix.len() + input.len());
    out.push_str(prefix);
    out.push_str(input);
    out
}

/// Run `n_inputs` embed calls against the dedicated embed context.
///
/// Each input is tokenised, encoded with `llama_encode`, and the
/// pooled per-sequence embedding read via
/// `llama_get_embeddings_seq`. KV cache is cleared between inputs so
/// independent inputs don't bleed into one another.
///
/// Matryoshka truncation: when `requested_dim` is `Some(n)` and `n <=
/// model_n_embd`, the leading `n` dimensions are returned (and
/// L2-renormalised so the truncated vector remains unit-norm — this
/// is the EmbeddingGemma MRL convention). When `n > model_n_embd` we
/// emit `InvalidRequest` so the caller knows the request is
/// unsatisfiable.
fn run_embed(
    state: &Arc<Mutex<State>>,
    inputs: &[String],
    requested_dim: Option<u32>,
    model_label: String,
) -> Result<EmbedResult, EmbedError> {
    let guard = state.lock().expect("poisoned llamacpp state mutex");
    let model = guard.model.as_ptr();
    let embed = guard.embed.as_ref().ok_or(EmbedError::Unsupported)?;
    let ctx = embed.ctx.ptr.as_ptr();
    let n_embd = embed.n_embd as usize;
    let n_ubatch = embed.n_ubatch as usize;

    if let Some(d) = requested_dim
        && d as usize > n_embd
    {
        return Err(EmbedError::InvalidRequest(format!(
            "dimensions {d} exceeds model n_embd {n_embd}"
        )));
    }
    let out_dim = requested_dim.map(|d| d as usize).unwrap_or(n_embd);

    // SAFETY: FFI; pointers held under the lock guard.
    let vocab = unsafe { ffi::llama_model_get_vocab(model) };

    let mut input_tokens: u32 = 0;
    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(inputs.len());

    for text in inputs {
        // Reset KV cache so each input starts at position 0.
        // SAFETY: FFI; ctx valid in scope.
        unsafe {
            let mem = ffi::llama_get_memory(ctx);
            if !mem.is_null() {
                ffi::llama_memory_clear(mem, true);
            }
        }

        // Tokenise. add_special=true so BOS/EOS markers the encoder
        // expects are emitted; parse_special=false because user input
        // shouldn't be interpreted as a control token.
        let mut tokens = tokenize(vocab, text.as_bytes(), true, false)
            .map_err(|_| EmbedError::InvalidRequest("tokenize failed".into()))?;
        if tokens.is_empty() {
            return Err(EmbedError::InvalidRequest(
                "input produced zero tokens".into(),
            ));
        }
        // Reject oversized inputs *before* calling llama_encode.
        // libllama asserts `n_ubatch >= n_tokens` inside the encoder
        // and aborts the whole process on failure (issue #20). The
        // structured error keeps the daemon alive and gives the
        // caller a per-input failure they can act on (truncate /
        // chunk / reject) instead of a closed connection.
        if tokens.len() > n_ubatch {
            return Err(EmbedError::InvalidRequest(format!(
                "input exceeds embed context: {} tokens > n_ubatch {}",
                tokens.len(),
                n_ubatch
            )));
        }
        input_tokens = input_tokens.saturating_add(tokens.len() as u32);

        // SAFETY: FFI; tokens.as_mut_ptr() valid for the call.
        let batch = unsafe { ffi::llama_batch_get_one(tokens.as_mut_ptr(), tokens.len() as i32) };
        // SAFETY: FFI; ctx valid.
        let rc = unsafe { ffi::llama_encode(ctx, batch) };
        if rc != 0 {
            return Err(EmbedError::Unavailable(format!(
                "llama_encode failed: {rc}"
            )));
        }

        // Read pooled embedding for sequence 0 (llama_batch_get_one
        // assigns all tokens to seq_id 0).
        // SAFETY: FFI; ctx valid; pointer is owned by libllama and
        // valid until the next encode/decode call.
        let raw = unsafe { ffi::llama_get_embeddings_seq(ctx, 0) };
        if raw.is_null() {
            return Err(EmbedError::Unavailable(
                "llama_get_embeddings_seq returned null".into(),
            ));
        }
        // SAFETY: FFI contract — `raw` points to `n_embd` consecutive
        // f32 values.
        let slice = unsafe { std::slice::from_raw_parts(raw, n_embd) };

        // Truncate (MRL) + L2-normalise.
        let mut vec: Vec<f32> = slice[..out_dim].to_vec();
        l2_normalise(&mut vec);
        embeddings.push(vec);
    }

    Ok(EmbedResult {
        embeddings,
        dimensions: out_dim as u32,
        model: model_label,
        usage: EmbedUsage { input_tokens },
    })
}

/// In-place L2 normalisation. Zero-norm vectors are left unchanged
/// (no division by zero).
fn l2_normalise(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The daemon can't resample and mtmd's audio entry point takes a
    // bare `&[f32]` with no rate argument, so a mismatched rate is
    // undetectable downstream: the encoder just reads the samples at
    // its own rate and returns a plausible, wrong answer. These pin
    // the rejection so that failure mode stays impossible.

    #[test]
    fn audio_rate_matching_expected_is_accepted() {
        assert!(check_audio_sample_rate("a1", 16_000, Some(16_000)).is_ok());
    }

    #[test]
    fn audio_rate_mismatch_is_rejected_naming_both_rates() {
        let err = check_audio_sample_rate("a1", 44_100, Some(16_000))
            .expect_err("44.1 kHz PCM must not reach a 16 kHz encoder");
        let msg = err.to_string();
        // The message has to carry both numbers: a consumer that only
        // learns "bad rate" can't tell what to resample *to*.
        assert!(msg.contains("44100"), "must name the declared rate: {msg}");
        assert!(msg.contains("16000"), "must name the required rate: {msg}");
        assert!(msg.contains("a1"), "must name the attachment: {msg}");
    }

    #[test]
    fn audio_rate_unknown_expectation_passes_through() {
        // An audio-capable mmproj that reports no rate gives us nothing
        // to compare against; rejecting here would break valid requests.
        assert!(check_audio_sample_rate("a1", 44_100, None).is_ok());
    }
}
