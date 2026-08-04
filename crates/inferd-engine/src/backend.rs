//! `Backend` trait and shared types.

use async_trait::async_trait;
use inferd_proto::embed::{EmbedResolved, EmbedUsage};
use inferd_proto::rerank::{RerankResolved, RerankResult as RerankScore, RerankUsage};
use inferd_proto::v2::{ResolvedV2, StopReasonV2, ToolCallId, ToolUseInput, UsageV2};
use std::pin::Pin;
use tokio_stream::Stream;

/// One event in a v2 generation stream — typed-content-block surface
/// per ADR 0015.
///
/// v2 separates user-visible text (`Text`) from reasoning trace
/// (`Thinking`) and emits complete tool-call requests (`ToolUse`) as
/// their own variant rather than raw tokens. Backends that don't
/// distinguish thinking content (any non-Gemma-4 backend) emit only
/// `Text` events.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenEventV2 {
    /// Incremental user-visible text.
    Text(String),
    /// Incremental reasoning trace (Gemma 4 `<|think|>` content).
    Thinking(String),
    /// Complete tool-call request emitted by the model.
    ToolUse {
        /// Identifier paired with the consumer's eventual ToolResult.
        tool_call_id: ToolCallId,
        /// Tool name from the request's `tools[]` table.
        name: String,
        /// JSON arguments emitted by the model.
        input: ToolUseInput,
    },
    /// Final event for a successful generation.
    Done {
        /// Reason generation stopped.
        stop_reason: StopReasonV2,
        /// Token-count usage.
        usage: UsageV2,
    },
}

/// Stream of `TokenEventV2` values produced by a backend during a v2
/// generation. Dropping the stream cancels the in-flight generation.
pub type TokenStreamV2 = Pin<Box<dyn Stream<Item = TokenEventV2> + Send>>;

/// Hardware-acceleration backend the engine adapter is built and
/// running with.
///
/// In a `dl-backends` build (v0.3 / ADR 0019), this is determined at
/// process start by probing the ggml backend registry once
/// `ggml_backend_load_all()` has dlopen'd the MODULE libs shipped
/// next to the daemon. The cascade is Metal → CUDA → ROCm → Vulkan →
/// CPU; the strongest backend the host actually has wins. The pick
/// can be overridden with `INFERD_FORCE_BACKEND={cpu|metal|cuda|rocm|vulkan}`.
///
/// In the v0.2.x static-build path, this reflects compile-time GGML
/// feature flags (`cuda` / `metal` / `vulkan` / `rocm` — at most one
/// is meaningful per build); pure CPU builds report `Cpu`.
///
/// A build *with* GPU support but where `n_gpu_layers == 0` also
/// effectively uses CPU at runtime — see [`AcceleratorInfo::gpu_layers`].
///
/// New variants may be added in future patch releases (NPU, etc.);
/// older subscribers should treat unknown variants as `Cpu` (the
/// fallback that's always safe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcceleratorKind {
    /// Built without any GPU/accelerator support.
    #[default]
    Cpu,
    /// Built with CUDA — NVIDIA GPU offload available.
    Cuda,
    /// Built with Metal — Apple GPU offload available.
    Metal,
    /// Built with Vulkan — cross-vendor GPU offload available.
    Vulkan,
    /// Built with HIP/ROCm — AMD GPU offload available.
    Rocm,
}

impl AcceleratorKind {
    /// Stable wire-form name: `"cpu"`, `"cuda"`, `"metal"`, `"vulkan"`,
    /// `"rocm"`. Used in admin status frames and `inferdctl doctor`.
    pub fn as_str(self) -> &'static str {
        match self {
            AcceleratorKind::Cpu => "cpu",
            AcceleratorKind::Cuda => "cuda",
            AcceleratorKind::Metal => "metal",
            AcceleratorKind::Vulkan => "vulkan",
            AcceleratorKind::Rocm => "rocm",
        }
    }
}

/// Snapshot of the active hardware-acceleration configuration.
///
/// `kind` is the chosen GGML backend (runtime-probed in `dl-backends`
/// builds; compile-time-pinned in v0.2.x static builds);
/// `gpu_layers` is the runtime configuration the adapter was
/// constructed with. A backend that probed `Cuda` but was configured
/// with `n_gpu_layers = 0` reports `kind = Cuda, gpu_layers = 0` —
/// i.e. CUDA-capable but currently CPU-bound. The distinction is
/// useful: it tells consumers the daemon *could* accelerate if
/// reconfigured, vs. there's no GPU module loaded at all.
///
/// `device_name` and `vram_total_bytes` are populated when the active
/// backend exposes at least one device through ggml's device API —
/// i.e. on `dl-backends` builds with a non-CPU backend selected. They
/// remain `None` on the CPU path and on adapters that don't go
/// through libllama (cloud / mock).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AcceleratorInfo {
    /// GGML backend the daemon picked.
    pub kind: AcceleratorKind,
    /// Layers offloaded to the accelerator at construction time. 0
    /// means CPU-only at runtime regardless of `kind`.
    pub gpu_layers: u32,
    /// Human-readable device name reported by ggml (`"NVIDIA GeForce
    /// RTX 4090"`, `"AMD Radeon RX 7900 XT"`, `"Apple M2 Pro"`, …).
    /// `None` on the CPU path or when the active backend exposes no
    /// device.
    pub device_name: Option<String>,
    /// Total VRAM (or unified memory budget on Apple Silicon) in
    /// bytes. `None` when the backend doesn't report a total. Free
    /// VRAM is intentionally omitted — it changes second-to-second
    /// and would force the capabilities frame to either lie or
    /// re-probe on every emit.
    pub vram_total_bytes: Option<u64>,
}

/// Per-backend capability advertisement. The daemon consults this on
/// boot to decide whether v2 multimodal / tool-use requests can be
/// dispatched, and reports the advertised set on the admin status
/// surface so middleware authors can introspect what the running
/// daemon can do without trial-and-error.
///
/// Per the v0.2 plan: until cloud adapters land, the only adapters
/// shipped are `mock` and `llamacpp`. Both opt-in selectively —
/// `mock` for tests, `llamacpp` once Phase 3+ wires mtmd / tool
/// parsing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendCapabilities {
    /// `true` if the backend implements `generate_v2` (typed
    /// content blocks, tool definitions). When `false` the daemon's
    /// v2 dispatch falls back to `Error{Internal,
    /// "v2 not supported by this backend"}`.
    pub v2: bool,
    /// `true` if the backend can ingest image attachments. Reported
    /// to consumers; requests with image content blocks against a
    /// non-image backend get `Error{AttachmentUnsupported,...}`.
    pub vision: bool,
    /// `true` if the backend can ingest audio attachments.
    pub audio: bool,
    /// Sample rate, in Hz, that this backend's audio encoder requires
    /// for `Attachment::Audio` payloads. `None` when the backend
    /// ingests no audio (`audio: false`) or cannot report a rate.
    ///
    /// This exists because the daemon does **not** resample (ADR 0016:
    /// the consumer decodes, and by extension owns rate conversion).
    /// Without advertising the rate there is nothing a consumer can
    /// introspect — sending 44.1 kHz PCM to a 16 kHz encoder is not an
    /// error the engine can detect from the bytes, so it produces
    /// silently wrong output rather than a failure. Publishing the
    /// required rate here, and rejecting a mismatched
    /// `Attachment::sample_rate`, turns that into a loud, fixable
    /// error. Backwards-additive: `None` for every backend that
    /// doesn't do audio.
    pub audio_sample_rate: Option<u32>,
    /// `true` if the backend can ingest video attachments. (Reserved.)
    pub video: bool,
    /// `true` if the backend natively supports tool-use round-tripping
    /// (parses `<|tool_call>` from token stream, accepts `tool_result`
    /// blocks in the next request, etc.).
    pub tools: bool,
    /// `true` if the backend separates `<|think|>` reasoning trace
    /// from user-visible output.
    pub thinking: bool,
    /// `true` if the backend implements `embed` (per ADR 0017). When
    /// `false` the daemon does not bind the embed socket; if it
    /// somehow gets bound and a request arrives, dispatch returns
    /// `Error{EmbedUnsupported}`.
    pub embed: bool,
    /// `true` if the backend implements `rerank` (per ADR 0027). When
    /// `false` the daemon does not bind the rerank socket; if it
    /// somehow gets bound and a request arrives, dispatch returns
    /// `Error{RerankUnsupported}`.
    ///
    /// Independent of [`Self::embed`]: reranking needs a cross-encoder
    /// with a classification head, which is a different model artefact
    /// from a bi-encoder embedder. A model that happened to support
    /// both would advertise both and bind both sockets; none currently
    /// shipped does.
    pub rerank: bool,
    /// Hardware-acceleration snapshot. `Cpu / 0` for the default
    /// trait impl; `mock` keeps the default; `llamacpp` reports the
    /// compile-time GGML backend + the configured `n_gpu_layers`.
    /// Reported on admin `status: capabilities` frames and in
    /// `inferdctl doctor` (#77).
    pub accelerator: AcceleratorInfo,
}

/// Result of a successful `Backend::embed()` call.
///
/// Embedding requests produce a single complete result, not a stream:
/// one vector per input string in the same order as the request's
/// `input`. `dimensions` is the actual length of each inner vector
/// after any MRL truncation.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedResult {
    /// One vector per input string, in input order. All inner vectors
    /// share the same length (`dimensions`).
    pub embeddings: Vec<Vec<f32>>,
    /// Actual length of each inner vector after any MRL truncation.
    pub dimensions: u32,
    /// Backend-reported model name (e.g. `"embeddinggemma-300m"`).
    pub model: String,
    /// Token-count usage.
    pub usage: EmbedUsage,
}

/// Errors returned by `Backend::embed()`.
///
/// Distinct from `GenerateError` because the embed surface has a
/// different error taxonomy (no streaming → no mid-stream concept;
/// adds `Unsupported` for the not-an-embed-backend case).
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// Backend was not ready when `embed()` was called.
    #[error("backend not ready")]
    NotReady,
    /// Backend doesn't expose embedding capability.
    #[error("embed not supported by this backend")]
    Unsupported,
    /// Backend rejected the request as malformed (dimensions out of
    /// range for the model, input too long for the context, etc.).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Backend tried to embed and failed (model not loaded, remote
    /// API errored, etc.).
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// Anything else.
    #[error("internal: {0}")]
    Internal(String),
}

/// Result of a successful `Backend::rerank()` call.
///
/// Like [`EmbedResult`], a single complete value rather than a stream —
/// a partial ordering is not useful.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankOutcome {
    /// One score per request document, **sorted descending by score**
    /// and already truncated to the request's `top_n`.
    ///
    /// Sorting and truncation happen here, in the adapter, rather than
    /// in the daemon: the adapter is the only layer that knows the
    /// score scale, and doing it once at the source means no consumer
    /// (including the daemon's own lifecycle) has to re-derive an
    /// ordering.
    pub results: Vec<RerankScore>,
    /// Backend-reported model name (e.g. `"bge-reranker-v2-m3"`).
    pub model: String,
    /// Token-count usage, summed across every query/document pair.
    pub usage: RerankUsage,
}

/// Errors returned by `Backend::rerank()`.
///
/// Mirrors [`EmbedError`] — same shape, separate type so each surface's
/// taxonomy can move independently.
#[derive(Debug, thiserror::Error)]
pub enum RerankError {
    /// Backend was not ready when `rerank()` was called.
    #[error("backend not ready")]
    NotReady,
    /// Backend doesn't expose reranking capability.
    #[error("rerank not supported by this backend")]
    Unsupported,
    /// Backend rejected the request as malformed (a query/document pair
    /// too long for the rerank context, etc.).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Backend tried to score and failed.
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// Anything else.
    #[error("internal: {0}")]
    Internal(String),
}

/// Errors returned by `Backend::generate()` *before* any tokens have streamed.
///
/// Mid-stream failures terminate the stream silently (no `Done` event); the
/// caller observes the absence of a terminal event and translates that to
/// `Response::Error` with `code: backend_unavailable` per `docs/protocol-v1.md`.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// Backend was not ready when `generate()` was called.
    #[error("backend not ready")]
    NotReady,
    /// Backend rejected the request as malformed (sampling out of range, etc.).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Backend tried to start generation and failed (model not loaded,
    /// remote API errored, etc.).
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// Anything else.
    #[error("internal: {0}")]
    Internal(String),
}

/// An inference backend.
///
/// Implementations are owned by the daemon and shared across requests through
/// `Arc<dyn Backend>`. Methods take `&self`; concurrent invocations of
/// `generate()` are serialised by the daemon's admission queue, not by the
/// trait.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Stable identifier for the backend, e.g. `"mock"`, `"llamacpp"`,
    /// `"anthropic"`. Echoed in `Response::Done::backend` for diagnostic
    /// purposes (ADR 0007).
    fn name(&self) -> &str;

    /// Whether the backend has finished its boot sequence and can serve
    /// requests. The daemon does not create its inference listener until
    /// every registered backend reports `true` (see `THREAT_MODEL.md` F-13).
    fn ready(&self) -> bool;

    /// Capabilities the backend advertises to the daemon and (via
    /// the admin status surface) to consumers. Default: text-only v1
    /// backend, no v2, no multimodal, no tools — matches the v0.1
    /// `mock` and `llamacpp` shape so existing implementors compile
    /// unchanged.
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    /// Begin a generation and return a stream of `TokenEventV2` values.
    ///
    /// As of v0.4 (ADR 0021) this is the single generation entry point —
    /// the old text-only v1 `generate(Resolved)` was removed when v1 was
    /// folded into v2. Text-only requests arrive as a v2 request whose
    /// content is a single `text` block.
    ///
    /// Errors returned here surface as `ResponseV2::Error` *before* any
    /// tokens reach the client. Errors after the first token terminate
    /// the stream without a `Done`.
    async fn generate_v2(&self, req: ResolvedV2) -> Result<TokenStreamV2, GenerateError>;

    /// Compute embeddings for the request's input strings (per
    /// ADR 0017). Default impl returns `EmbedError::Unsupported` —
    /// adapters opt in by overriding and setting
    /// `capabilities().embed = true`. The daemon binds the embed
    /// socket only when the active backend's capability is `true`,
    /// so reaching this default impl in production is a fail-safe
    /// for misconfiguration.
    async fn embed(&self, _req: EmbedResolved) -> Result<EmbedResult, EmbedError> {
        Err(EmbedError::Unsupported)
    }

    /// Score each document against the query with a cross-encoder (per
    /// ADR 0027). Default impl returns `RerankError::Unsupported` —
    /// adapters opt in by overriding and setting
    /// `capabilities().rerank = true`. As with `embed`, the daemon binds
    /// the rerank socket only when the capability is `true`, so reaching
    /// this default in production is a fail-safe for misconfiguration.
    ///
    /// Implementations return results already sorted descending by score
    /// and truncated to `req.top_n`.
    async fn rerank(&self, _req: RerankResolved) -> Result<RerankOutcome, RerankError> {
        Err(RerankError::Unsupported)
    }

    /// Best-effort graceful shutdown. The daemon calls this on stop; the
    /// adapter should release model memory, terminate worker threads, and
    /// any other long-lived resources within the deadline.
    async fn stop(&self, _timeout: std::time::Duration) -> Result<(), GenerateError> {
        Ok(())
    }
}
