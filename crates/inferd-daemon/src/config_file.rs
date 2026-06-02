//! Operator-facing JSON config file.
//!
//! Default location: `~/.inferd/config.json` (Unix) /
//! `%USERPROFILE%\.inferd\config.json` (Windows). Override via
//! `--config` CLI flag or `INFERD_CONFIG` env var.
//!
//! # Schema (single-backend, legacy)
//!
//! ```json
//! {
//!   "auto_pull": true,
//!   "models_home": "~/.local/share/models",
//!   "model": {
//!     "name":       "gemma-4-e4b",
//!     "sha256":     "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
//!     "size_bytes": 5126304928,
//!     "source_url": "https://huggingface.co/unsloth/.../resolve/main/...gguf",
//!     "license":    "apache-2.0"
//!   },
//!   "n_ctx":         8192,
//!   "n_gpu_layers":  0,
//!   "admin_addr":    "/run/inferd/admin.sock"
//! }
//! ```
//!
//! # Schema (multi-backend, v0.2+)
//!
//! Per ADR 0007, the router walks an *ordered* list of backends; the
//! first that's `ready()` and not currently circuit-broken serves the
//! request. The config-file surface mirrors that:
//!
//! ```json
//! {
//!   "models_home": "~/.local/share/models",
//!   "backends": [
//!     {
//!       "kind": "llamacpp",
//!       "name": "local-gemma",
//!       "model":  { "name": "gemma-4-e4b", "sha256": "...", "source_url": "https://...gguf" },
//!       "mmproj": { "name": "gemma-4-e4b-mmproj", "sha256": "...", "source_url": "https://...mmproj.gguf" },
//!       "n_ctx": 8192,
//!       "n_gpu_layers": 35
//!     },
//!     {
//!       "kind": "openai-compat",
//!       "name": "anthropic-fallback",
//!       "base_url": "https://api.anthropic.com",
//!       "model": "claude-opus-4-7",
//!       "api_key_env": "ANTHROPIC_API_KEY",
//!       "timeout_secs": 300
//!     }
//!   ]
//! }
//! ```
//!
//! `backends:` and `model:` are mutually exclusive. When `model:` is
//! present (legacy single-backend shape), the daemon promotes it to a
//! one-element `backends:` list with `kind: "llamacpp"` so existing
//! v0.1.x configs keep working without edits.
//!
//! API keys for `openai-compat` are referenced by env-var **name**
//! via `api_key_env:` — never embedded literally in the file. The
//! daemon reads the named env at startup. When `api_key_env:` is
//! absent, falls back to `INFERD_OPENAI_API_KEY`, then `OPENAI_API_KEY`,
//! then empty (skips `Authorization` for self-hosted endpoints).
//!
//! The `kind:` field is an open-ended tagged union: future variants
//! (`bedrock-invoke`, `bedrock-converse`, etc.) slot in additively
//! without breaking existing configs.
//!
//! Resolution order for the model store (per ADR 0011):
//!
//! 1. `models_home` field if set in this config.
//! 2. `MODELS_HOME` env var.
//! 3. Platform default (XDG / Application Support / LOCALAPPDATA).
//!
//! CLI flags override config-file values when both are present.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

/// Top-level config-file schema.
///
/// Two flavours coexist:
///
/// - **Legacy single-backend** — `model:` at the top level, plus
///   `n_ctx` / `n_gpu_layers`. Implies one `kind: "llamacpp"` backend.
/// - **Multi-backend** — `backends: [...]` carries an ordered list of
///   backend entries. Router walks the list per ADR 0007.
///
/// The two are mutually exclusive at parse time: setting both is a
/// validation error. `auto_pull` and `admin_addr` apply to both
/// flavours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// When `true` and a `kind: "llamacpp"` model file is absent, the
    /// daemon downloads it from the entry's `source_url` on startup.
    /// When `false`, the daemon refuses to start with a clear error
    /// pointing at the operator's next step. Default: `true`. Applies
    /// to every llamacpp entry in `backends:`.
    #[serde(default = "default_auto_pull")]
    pub auto_pull: bool,

    /// Override for the shared model store root. When unset the
    /// daemon falls back to `MODELS_HOME` env, then the platform
    /// default. Tilde-expanded on read.
    #[serde(default)]
    pub models_home: Option<PathBuf>,

    /// Legacy single-backend model spec. Deprecated in favour of
    /// `backends:` but kept for v0.1.x config-file compatibility.
    /// Mutually exclusive with `backends:`.
    #[serde(default)]
    pub model: Option<ModelConfig>,

    /// Llama.cpp context window in tokens. Default: 8192. Used as
    /// the fallback for legacy `model:` entries; multi-backend
    /// entries carry their own `n_ctx`.
    #[serde(default = "default_n_ctx")]
    pub n_ctx: u32,

    /// Llama.cpp GPU layer offload count. 0 = CPU-only. Default: 0.
    /// Used as the fallback for legacy `model:` entries; multi-
    /// backend entries carry their own `n_gpu_layers`.
    #[serde(default)]
    pub n_gpu_layers: i32,

    /// Admin socket address. Default: platform-specific path per
    /// `docs/protocol-v1.md` §"Admin endpoint".
    #[serde(default)]
    pub admin_addr: Option<String>,

    /// Ordered list of backends (multi-backend shape). First entry
    /// is highest priority — the router tries it first, then the
    /// next, etc. Mutually exclusive with `model:`.
    #[serde(default)]
    pub backends: Option<Vec<BackendEntry>>,

    /// Optional listener overrides. Default behaviour is unchanged:
    /// the operator picks a transport via `--tcp` / `--uds` /
    /// `--pipe` on the CLI. When `listen:` is present **and** the
    /// CLI did not pass a transport flag, the daemon binds the
    /// transports declared here. CLI flags always win when both
    /// are set. Restart-time only — no config watcher.
    #[serde(default)]
    pub listen: Option<ListenConfig>,
}

/// Operator-declared listener overrides. Every field is optional.
/// TCP is **off by default** — set `tcp:` (and `tcp_v2:` if running
/// with v2) to opt in for cross-VM use cases (WSL ↔ Windows host,
/// podman-on-machine, …) where Unix sockets / named pipes don't
/// cross the boundary cleanly. Mirrors the security shape of
/// `openai-compat`: the API key is referenced by env-var **name**,
/// never embedded literally in the file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListenConfig {
    /// Loopback TCP bind address for the v1 inference socket, e.g.
    /// `"127.0.0.1:9090"` or `"0.0.0.0:9090"`. When unset, no v1
    /// TCP listener is bound from config (CLI `--tcp` may still
    /// provide one). v0.1 invariant: CLI mutual exclusion still
    /// applies — if CLI passes `--uds` / `--pipe`, the config
    /// `tcp:` is ignored with a one-line warning at startup.
    #[serde(default)]
    pub tcp: Option<String>,

    /// Loopback TCP bind address for the v2 inference socket. Has
    /// no effect unless `--v2` is also set on the CLI.
    #[serde(default)]
    pub tcp_v2: Option<String>,

    /// Loopback TCP bind address for the embed socket per ADR 0017.
    /// Has no effect unless `--embed` is also set on the CLI and the
    /// active backend advertises `capabilities().embed == true`.
    #[serde(default)]
    pub tcp_embed: Option<String>,

    /// **Name** of the env var carrying the pre-shared API key for
    /// TCP clients (THREAT_MODEL F-8). When set, the daemon reads
    /// the named env at startup and clients must send
    /// `{"type":"auth","key":"<value>"}` as their first NDJSON
    /// frame. UDS and named-pipe transports ignore this — kernel-
    /// attested peer credentials (F-7) gate those. CLI `--api-key`
    /// always wins when both are set.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

/// A single backend declaration. Tagged on `kind:` so future
/// variants (`bedrock-converse`, …) slot in additively. Unknown
/// kinds are rejected at parse time so operators see a clear error
/// rather than a silent skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BackendEntry {
    /// Local llama.cpp backend over a GGUF file in the shared CAS.
    Llamacpp(LlamacppEntry),
    /// Outbound HTTPS adapter for any provider speaking the OpenAI
    /// Chat Completions wire (OpenAI, Anthropic via the compat layer
    /// at `api.anthropic.com/v1/`, OpenRouter, vLLM, LM Studio,
    /// LocalAI, Ollama, llama.cpp's HTTP server).
    OpenaiCompat(OpenaiCompatEntry),
    /// AWS Bedrock-runtime
    /// `InvokeModelWithResponseStream` adapter
    /// (Phase 6B-5). v0.2.0 ships only the Anthropic-on-Bedrock body
    /// shape — Claude models invoked via Bedrock's pinned
    /// `anthropic_version: "bedrock-2023-05-31"` payload.
    BedrockInvoke(BedrockInvokeEntry),
}

impl BackendEntry {
    /// Operator-supplied stable identifier, used in router feedback
    /// and admin-status events.
    pub fn name(&self) -> &str {
        match self {
            BackendEntry::Llamacpp(e) => &e.name,
            BackendEntry::OpenaiCompat(e) => &e.name,
            BackendEntry::BedrockInvoke(e) => &e.name,
        }
    }
}

/// Llamacpp backend entry inside `backends:`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlamacppEntry {
    /// Stable operator-facing identifier, e.g. `"local-gemma"`. Used
    /// in router feedback + admin events. Required to be unique
    /// across all entries.
    pub name: String,

    /// Per-entry model spec (CAS layout, ADR 0011).
    pub model: ModelConfig,

    /// Optional multimodal projector (mmproj) for v2 vision/audio
    /// (ADR 0013 / 0015 / 0016). When set, the daemon fetches the
    /// projector as an additional CAS blob — same pinned-URL +
    /// constant-time-SHA path as `model` — and hands it to libmtmd, so
    /// the backend's `capabilities().vision` flips `true` and v2 image
    /// attachments are encoded. When `None` (the default), the backend
    /// is text-only and reports `vision: false`. The projector must
    /// match the base model family (e.g. a Gemma 4 mmproj for a Gemma 4
    /// model). Issue #30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<ModelConfig>,

    /// Llama.cpp context window in tokens. Default: 8192.
    #[serde(default = "default_n_ctx")]
    pub n_ctx: u32,

    /// Llama.cpp GPU layer offload count. 0 = CPU-only. Default: 0.
    #[serde(default)]
    pub n_gpu_layers: i32,

    /// Opt this backend into serving embeddings per ADR 0017. When
    /// `true`, the adapter allocates a *second* `llama_context`
    /// configured with `embeddings = true` so embed requests don't
    /// race the generation context. `capabilities().embed` flips
    /// `true` accordingly. Default: `false`.
    #[serde(default)]
    pub embed: bool,

    /// Pooling strategy for the embedding context, mapped 1:1 to
    /// llama.cpp's `enum llama_pooling_type`. Most embedding models
    /// expect `1` (`LLAMA_POOLING_TYPE_MEAN`), which is the default;
    /// EmbeddingGemma 300M is in this group. Set explicitly only if
    /// the model documents a different strategy (e.g. `2` =
    /// `CLS`, `3` = `LAST`). Ignored when `embed = false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_pooling: Option<i32>,

    /// Context window for the dedicated embedding `llama_context`,
    /// in tokens. Embedding models typically have a smaller window
    /// than generation models — 2048 is the EmbeddingGemma 300M
    /// default and is what the adapter uses when this is unset.
    /// Ignored when `embed = false`.
    #[serde(default = "default_embed_n_ctx")]
    pub embed_n_ctx: u32,
}

/// OpenAI-compat backend entry inside `backends:`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiCompatEntry {
    /// Stable operator-facing identifier, e.g. `"anthropic-fallback"`.
    pub name: String,

    /// Base URL of the upstream, no trailing slash and no path
    /// (the adapter appends `/v1/chat/completions`). Examples:
    /// `https://api.openai.com`, `https://api.anthropic.com`,
    /// `http://localhost:11434`.
    pub base_url: String,

    /// Upstream model identifier echoed in the request `model` field.
    /// Provider-specific (e.g. `gpt-4o-mini`, `claude-opus-4-7`,
    /// `llama3.1:8b`).
    pub model: String,

    /// **Name** of the env var carrying the bearer token — never the
    /// literal token. Operators set the env separately so secrets
    /// stay out of the config file. When unset, the daemon falls
    /// back to `INFERD_OPENAI_API_KEY`, then `OPENAI_API_KEY`, then
    /// skips the `Authorization` header (some self-hosted endpoints
    /// accept unauthenticated traffic).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Total request timeout in seconds. Default 300 (5 minutes).
    #[serde(default = "default_openai_timeout_secs")]
    pub timeout_secs: u64,
}

/// Bedrock-invoke backend entry inside `backends:`.
///
/// Auth precedence at startup (mirrors `openai-compat` env-var-by-name
/// shape so secrets stay out of the file):
///
/// 1. `bearer_token_env: "<NAME>"` — when the named env contains a
///    non-empty value, the adapter sends `Authorization: Bearer
///    <value>` and skips SigV4. Mirrors AWS' 2025-06
///    `AWS_BEARER_TOKEN_BEDROCK` rollout.
/// 2. SigV4 against the standard AWS credential chain — env vars
///    `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (+ optional
///    `AWS_SESSION_TOKEN`). Cross-account assume-role is out of
///    scope for v0.2.0; operators set the env vars from their own
///    session before starting the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BedrockInvokeEntry {
    /// Stable operator-facing identifier, e.g. `"bedrock-claude"`.
    pub name: String,

    /// AWS region the Bedrock endpoint lives in, e.g. `"us-east-1"`.
    /// Used for both the endpoint host and SigV4 signing scope.
    pub region: String,

    /// Bedrock model id, e.g.
    /// `"anthropic.claude-3-5-sonnet-20241022-v2:0"`. URL-encoded
    /// by the adapter.
    pub model_id: String,

    /// Optional **name** of the env var carrying the Bedrock bearer
    /// token (`AWS_BEARER_TOKEN_BEDROCK` shape) — never the literal
    /// token. When the named env is non-empty, bearer auth wins and
    /// SigV4 is skipped. When unset or empty, the adapter falls back
    /// to the standard `AWS_ACCESS_KEY_ID` /
    /// `AWS_SECRET_ACCESS_KEY` chain.
    #[serde(default)]
    pub bearer_token_env: Option<String>,

    /// Optional endpoint host override. Empty/absent → default
    /// `bedrock-runtime.<region>.amazonaws.com`. Useful for VPC
    /// endpoints / integration tests.
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Total request timeout in seconds. Default 300 (5 minutes).
    #[serde(default = "default_bedrock_timeout_secs")]
    pub timeout_secs: u64,
}

/// Per-model entry: pinned URL + pinned SHA-256 + name.
///
/// The shape mirrors `fetch::ModelSpec` but as a serde-deserialisable
/// config-file type. Conversion is straightforward (`From` impl below).
///
/// Note: there is no `filename` field. The blob's on-disk location
/// is derived from its SHA-256 (CAS layout, ADR 0011); the manifest
/// at `<store>/manifests/<name>.json` is the only place a name maps
/// to a blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Stable identifier, e.g. `"gemma-4-e4b"`. Used as the manifest
    /// filename and the lock-file basename.
    pub name: String,
    /// Lowercase hex SHA-256 of the GGUF bytes. Required.
    pub sha256: String,
    /// Advisory total size for progress reporting + manifest.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Direct-download HTTPS endpoint. Must be `https://`.
    pub source_url: String,
    /// SPDX-style license id when known. Recorded in the manifest.
    #[serde(default)]
    pub license: Option<String>,
}

fn default_auto_pull() -> bool {
    true
}

fn default_n_ctx() -> u32 {
    8192
}

fn default_embed_n_ctx() -> u32 {
    2048
}

fn default_openai_timeout_secs() -> u64 {
    300
}

fn default_bedrock_timeout_secs() -> u64 {
    300
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

/// Default config-file path: `~/.inferd/config.json` on Unix /
/// `%USERPROFILE%\.inferd\config.json` on Windows. Honours
/// `INFERD_CONFIG` for tests and ops.
pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("INFERD_CONFIG") {
        return PathBuf::from(p);
    }
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".inferd").join("config.json")
}

/// Default `ConfigFile` shipped with first-boot installs. Two
/// llamacpp backends — `gemma-4-e4b` for generation and
/// `embeddinggemma-300m` for embeddings — both with `auto_pull: true`
/// so a clean machine fetches the blobs into the CAS on first boot.
///
/// SHAs are pinned against `x-linked-etag` from HuggingFace's resolve
/// endpoint at the time this default was authored; the daemon's
/// fetch path verifies them with a constant-time compare per the
/// invariant in `context.md`.
///
/// Operators who want to customise (different model, GPU offload,
/// extra cloud entries, …) edit the file after first boot — the
/// daemon never overwrites an existing config.
pub fn default_first_boot_config() -> ConfigFile {
    // With dl-backends the accelerator is picked at runtime (Metal / CUDA /
    // CPU); -1 means "offload all layers" which is correct for any GPU path.
    // On a CPU-only host the value is harmless — llama.cpp clamps it to 0
    // when no GPU device is present.  Operators who want to force CPU can
    // set n_gpu_layers: 0 in ~/.inferd/config.json after first boot.
    //
    // Without dl-backends a single accelerator is compiled in.  Defaulting
    // to 0 keeps the existing v0.2.x behaviour (static CPU build).
    #[cfg(feature = "dl-backends")]
    let default_gpu_layers: i32 = -1;
    #[cfg(not(feature = "dl-backends"))]
    let default_gpu_layers: i32 = 0;

    ConfigFile {
        auto_pull: true,
        models_home: None,
        model: None,
        n_ctx: default_n_ctx(),
        n_gpu_layers: default_gpu_layers,
        admin_addr: None,
        backends: Some(vec![
            BackendEntry::Llamacpp(LlamacppEntry {
                name: "gemma-4-e4b".into(),
                model: ModelConfig {
                    name: "gemma-4-e4b".into(),
                    sha256: "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36"
                        .into(),
                    size_bytes: Some(5_126_304_928),
                    source_url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/\
                         gemma-4-E4B-it-UD-Q4_K_XL.gguf"
                        .into(),
                    license: Some("gemma".into()),
                },
                // Gemma 4 is natively multimodal; unsloth ships the
                // vision projector in the same repo as the text GGUF.
                // Pull it by default (issue #30) so a fresh install has
                // working v2 image attachments out of the box — the
                // daemon fetches it as a second CAS blob and hands it to
                // mtmd, flipping capabilities().vision = true. mmproj-F16
                // is the standard quant (~945 MB). Operators who want a
                // text-only daemon can delete this block after first
                // boot.
                mmproj: Some(ModelConfig {
                    name: "gemma-4-e4b-mmproj".into(),
                    sha256: "ddf46c21d7078e95338cfc22306b19b276a29a5ad089023449dd54d4b6170a51"
                        .into(),
                    size_bytes: Some(990_372_672),
                    source_url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/\
                         mmproj-F16.gguf"
                        .into(),
                    license: Some("gemma".into()),
                }),
                n_ctx: default_n_ctx(),
                n_gpu_layers: default_gpu_layers,
                embed: false,
                embed_pooling: None,
                embed_n_ctx: default_embed_n_ctx(),
            }),
            BackendEntry::Llamacpp(LlamacppEntry {
                name: "embeddinggemma-300m".into(),
                model: ModelConfig {
                    name: "embeddinggemma-300m".into(),
                    sha256: "a0f7b4e13c397a6e1b32c2de75b1f65a14c92ec524d5f674d94a4290a1c4969b"
                        .into(),
                    size_bytes: Some(328_577_056),
                    source_url:
                        "https://huggingface.co/unsloth/embeddinggemma-300m-GGUF/resolve/main/\
                         embeddinggemma-300M-Q8_0.gguf"
                            .into(),
                    license: Some("gemma".into()),
                },
                // Embedding model — no projector.
                mmproj: None,
                n_ctx: default_embed_n_ctx(),
                n_gpu_layers: default_gpu_layers,
                embed: true,
                embed_pooling: None,
                embed_n_ctx: default_embed_n_ctx(),
            }),
        ]),
        listen: None,
    }
}

/// Write the default config to `path` if `path` does not already
/// exist. Creates the parent directory if needed. **Never** overwrites
/// an existing file — operator customisations win.
///
/// Returns `Ok(true)` if a default was written, `Ok(false)` if the
/// path already had a file. I/O errors propagate so callers can log
/// them; they are not fatal — the daemon proceeds to load whatever
/// was on disk (or fall back to CLI flags + mock if nothing was).
pub fn write_default_if_missing(path: &Path) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cfg = default_first_boot_config();
    let tmp = path.with_extension("json.tmp");
    {
        let file = File::create(&tmp)?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &cfg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::io::Write::write_all(&mut writer, b"\n")?;
        std::io::Write::flush(&mut writer)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Errors produced by `ConfigFile::load`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file did not exist at the resolved path.
    #[error("config file not found: {0}")]
    NotFound(PathBuf),
    /// I/O error reading the file.
    #[error("io reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// JSON parse failure.
    #[error("parse {path}: {source}")]
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// Validation failure on otherwise-well-formed config.
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl ConfigFile {
    /// Read + parse + validate a config file at `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                ConfigError::NotFound(path.to_path_buf())
            } else {
                ConfigError::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })?;
        let reader = BufReader::new(file);
        let mut cfg: ConfigFile =
            serde_json::from_reader(reader).map_err(|e| ConfigError::Parse {
                path: path.to_path_buf(),
                source: e,
            })?;
        cfg.expand_paths();
        cfg.validate()?;
        Ok(cfg)
    }

    fn expand_paths(&mut self) {
        if let Some(p) = self.models_home.as_ref()
            && let Some(stripped) = p
                .to_str()
                .and_then(|s| s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")))
            && let Some(home) = home_dir()
        {
            self.models_home = Some(home.join(stripped));
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        match (&self.model, &self.backends) {
            (Some(_), Some(_)) => {
                return Err(ConfigError::Invalid(
                    "config: `model` and `backends` are mutually exclusive — \
                     pick one shape, not both"
                        .into(),
                ));
            }
            (None, None) => {
                return Err(ConfigError::Invalid(
                    "config: must specify either `model` (legacy single-backend) \
                     or `backends` (multi-backend list)"
                        .into(),
                ));
            }
            _ => {}
        }
        if self.n_ctx == 0 {
            return Err(ConfigError::Invalid("n_ctx must be > 0".into()));
        }
        if let Some(m) = &self.model {
            validate_model_config(m)?;
        }
        if let Some(listen) = &self.listen {
            if let Some(addr) = &listen.tcp
                && addr.trim().is_empty()
            {
                return Err(ConfigError::Invalid(
                    "listen.tcp must not be empty when set".into(),
                ));
            }
            if let Some(addr) = &listen.tcp_v2
                && addr.trim().is_empty()
            {
                return Err(ConfigError::Invalid(
                    "listen.tcp_v2 must not be empty when set".into(),
                ));
            }
            if let Some(addr) = &listen.tcp_embed
                && addr.trim().is_empty()
            {
                return Err(ConfigError::Invalid(
                    "listen.tcp_embed must not be empty when set".into(),
                ));
            }
        }
        if let Some(list) = &self.backends {
            if list.is_empty() {
                return Err(ConfigError::Invalid(
                    "backends list must not be empty".into(),
                ));
            }
            let mut seen = std::collections::HashSet::with_capacity(list.len());
            for entry in list {
                let name = entry.name();
                if name.is_empty() {
                    return Err(ConfigError::Invalid(
                        "backends[].name must not be empty".into(),
                    ));
                }
                if !seen.insert(name.to_string()) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate backends[].name {name:?} — names must be unique"
                    )));
                }
                match entry {
                    BackendEntry::Llamacpp(e) => {
                        validate_model_config(&e.model)?;
                        // mmproj (issue #30) is fetched + SHA-verified
                        // through the same path as the base model, so it
                        // gets the same https + 64-hex-sha validation.
                        if let Some(mm) = e.mmproj.as_ref() {
                            validate_model_config(mm)?;
                        }
                        if e.n_ctx == 0 {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].n_ctx must be > 0"
                            )));
                        }
                    }
                    BackendEntry::OpenaiCompat(e) => {
                        if e.base_url.trim().is_empty() {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].base_url must not be empty"
                            )));
                        }
                        if !(e.base_url.starts_with("https://")
                            || e.base_url.starts_with("http://"))
                        {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].base_url must be http:// or https:// \
                                 (got {:?})",
                                e.base_url
                            )));
                        }
                        if e.model.trim().is_empty() {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].model must not be empty"
                            )));
                        }
                        if e.timeout_secs == 0 {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].timeout_secs must be > 0"
                            )));
                        }
                    }
                    BackendEntry::BedrockInvoke(e) => {
                        if e.region.trim().is_empty() {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].region must not be empty"
                            )));
                        }
                        if e.model_id.trim().is_empty() {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].model_id must not be empty"
                            )));
                        }
                        if e.timeout_secs == 0 {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].timeout_secs must be > 0"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Canonical multi-backend list. When the operator wrote the
    /// legacy single-backend shape (`model:` at top level), this
    /// returns a one-element list with `kind: "llamacpp"` so the
    /// rest of the daemon only ever sees the multi-backend shape.
    pub fn resolved_backends(&self) -> Vec<BackendEntry> {
        if let Some(list) = &self.backends {
            return list.clone();
        }
        // Legacy promotion path. `validate()` ensures exactly one of
        // (`model`, `backends`) is set, so the unwrap is unreachable
        // for any value that reached this method.
        let m = self
            .model
            .as_ref()
            .expect("validate() guarantees one of model|backends is set")
            .clone();
        vec![BackendEntry::Llamacpp(LlamacppEntry {
            name: m.name.clone(),
            model: m,
            // Legacy single-model configs are text-only; vision needs
            // the multi-backend `backends:` shape with an `mmproj` block
            // (issue #30).
            mmproj: None,
            n_ctx: self.n_ctx,
            n_gpu_layers: self.n_gpu_layers,
            // Legacy single-model configs predate ADR 0017's embed
            // surface and stay generation-only. Operators wanting
            // embeddings migrate to the multi-backend `backends:`
            // shape.
            embed: false,
            embed_pooling: None,
            embed_n_ctx: default_embed_n_ctx(),
        })]
    }
}

fn validate_model_config(m: &ModelConfig) -> Result<(), ConfigError> {
    if m.name.is_empty() {
        return Err(ConfigError::Invalid("model.name must not be empty".into()));
    }
    if !m.source_url.starts_with("https://") {
        return Err(ConfigError::Invalid(format!(
            "model.source_url must be https:// (got {:?})",
            m.source_url
        )));
    }
    if m.sha256.len() != 64
        || !m
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(ConfigError::Invalid(
            "model.sha256 must be 64 lowercase hex chars".into(),
        ));
    }
    Ok(())
}

impl From<&ModelConfig> for crate::fetch::ModelSpec {
    fn from(m: &ModelConfig) -> Self {
        crate::fetch::ModelSpec {
            name: m.name.clone(),
            source_url: m.source_url.clone(),
            sha256_hex: m.sha256.clone(),
            size_bytes: m.size_bytes,
            license: m.license.clone(),
            source: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_config(s: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn good_json() -> String {
        r#"{
            "auto_pull": true,
            "models_home": "/tmp/inferd-models-home",
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "size_bytes": 5126304928,
                "source_url": "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/gemma-4-E4B-it-UD-Q4_K_XL.gguf",
                "license": "apache-2.0"
            },
            "n_ctx": 8192,
            "n_gpu_layers": 0
        }"#
        .to_string()
    }

    #[test]
    fn load_well_formed_config() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let m = cfg.model.as_ref().expect("legacy model present");
        assert_eq!(m.name, "gemma-4-e4b");
        assert_eq!(m.size_bytes, Some(5_126_304_928));
        assert_eq!(m.license.as_deref(), Some("apache-2.0"));
        assert!(cfg.auto_pull);
        assert_eq!(cfg.n_ctx, 8192);
        assert_eq!(
            cfg.models_home,
            Some(PathBuf::from("/tmp/inferd-models-home"))
        );
    }

    #[test]
    fn missing_file_returns_not_found() {
        let path = std::env::temp_dir().join("inferd-config-does-not-exist.json");
        let _ = std::fs::remove_file(&path);
        let err = ConfigFile::load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::NotFound(_)));
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let f = write_config("{ not valid json");
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn http_url_rejected() {
        let bad = good_json().replace("https://", "http://");
        let f = write_config(&bad);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("https://")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn uppercase_sha_rejected() {
        let bad = good_json().replace(
            "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
            "30D1E7949597A3446726064E80B876FD1B5CBA4AA6EEC53D27AFA420E731FB36",
        );
        let f = write_config(&bad);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("lowercase hex")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn short_sha_rejected() {
        let bad = good_json().replace(
            "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
            "30d1e7",
        );
        let f = write_config(&bad);
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn defaults_when_optional_fields_missing() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            }
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        let m = cfg.model.as_ref().expect("legacy model present");
        assert!(cfg.auto_pull);
        assert_eq!(cfg.n_ctx, 8192);
        assert_eq!(cfg.n_gpu_layers, 0);
        assert!(m.size_bytes.is_none());
        assert!(cfg.models_home.is_none());
        assert!(m.license.is_none());
    }

    #[test]
    fn modelconfig_converts_to_fetch_modelspec() {
        let cfg = ModelConfig {
            name: "x".into(),
            sha256: "abc".into(),
            size_bytes: Some(42),
            source_url: "https://e/x.gguf".into(),
            license: Some("mit".into()),
        };
        let spec: crate::fetch::ModelSpec = (&cfg).into();
        assert_eq!(spec.name, "x");
        assert_eq!(spec.size_bytes, Some(42));
        assert_eq!(spec.sha256_hex, "abc");
        assert_eq!(spec.license.as_deref(), Some("mit"));
    }

    fn good_multi_backend_json() -> String {
        r#"{
            "models_home": "/tmp/inferd-models-home",
            "backends": [
                {
                    "kind": "llamacpp",
                    "name": "local-gemma",
                    "model": {
                        "name": "gemma-4-e4b",
                        "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                        "source_url": "https://example.com/gemma.gguf"
                    },
                    "n_ctx": 8192,
                    "n_gpu_layers": 35
                },
                {
                    "kind": "openai-compat",
                    "name": "anthropic-fallback",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-opus-4-7",
                    "api_key_env": "ANTHROPIC_API_KEY"
                }
            ]
        }"#
        .to_string()
    }

    #[test]
    fn load_multi_backend_config() {
        let f = write_config(&good_multi_backend_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        assert!(cfg.model.is_none());
        let list = cfg.backends.as_ref().expect("backends present");
        assert_eq!(list.len(), 2);
        match &list[0] {
            BackendEntry::Llamacpp(e) => {
                assert_eq!(e.name, "local-gemma");
                assert_eq!(e.model.name, "gemma-4-e4b");
                assert_eq!(e.n_ctx, 8192);
                assert_eq!(e.n_gpu_layers, 35);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
        match &list[1] {
            BackendEntry::OpenaiCompat(e) => {
                assert_eq!(e.name, "anthropic-fallback");
                assert_eq!(e.base_url, "https://api.anthropic.com");
                assert_eq!(e.model, "claude-opus-4-7");
                assert_eq!(e.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
                assert_eq!(e.timeout_secs, 300);
            }
            other => panic!("expected openai-compat, got {other:?}"),
        }
    }

    #[test]
    fn rejects_both_model_and_backends() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            },
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "x",
                    "base_url": "https://api.openai.com",
                    "model": "gpt-4o-mini"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("mutually exclusive")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_neither_model_nor_backends() {
        let json = r#"{ "auto_pull": true }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("must specify either")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_backends_list() {
        let json = r#"{ "backends": [] }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("must not be empty")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_backend_names() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "dup",
                    "base_url": "https://api.openai.com",
                    "model": "gpt-4o-mini"
                },
                {
                    "kind": "openai-compat",
                    "name": "dup",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-opus-4-7"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("duplicate")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_openai_compat_without_base_url() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "x",
                    "base_url": "",
                    "model": "gpt-4o-mini"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_openai_compat_with_bad_scheme() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "x",
                    "base_url": "ftp://api.openai.com",
                    "model": "gpt-4o-mini"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("http")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn accepts_openai_compat_with_localhost_http() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "ollama",
                    "base_url": "http://localhost:11434",
                    "model": "llama3.1:8b"
                }
            ]
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        assert_eq!(cfg.resolved_backends().len(), 1);
    }

    #[test]
    fn rejects_unknown_kind() {
        let json = r#"{
            "backends": [
                {
                    "kind": "future-thing-not-supported",
                    "name": "x"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn loads_bedrock_invoke_entry() {
        let json = r#"{
            "backends": [
                {
                    "kind": "bedrock-invoke",
                    "name": "bedrock-claude",
                    "region": "us-east-1",
                    "model_id": "anthropic.claude-3-5-sonnet-20241022-v2:0",
                    "bearer_token_env": "AWS_BEARER_TOKEN_BEDROCK"
                }
            ]
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        let list = cfg.backends.as_ref().unwrap();
        assert_eq!(list.len(), 1);
        match &list[0] {
            BackendEntry::BedrockInvoke(e) => {
                assert_eq!(e.name, "bedrock-claude");
                assert_eq!(e.region, "us-east-1");
                assert_eq!(e.model_id, "anthropic.claude-3-5-sonnet-20241022-v2:0");
                assert_eq!(
                    e.bearer_token_env.as_deref(),
                    Some("AWS_BEARER_TOKEN_BEDROCK")
                );
                assert!(e.endpoint.is_none());
                assert_eq!(e.timeout_secs, 300);
            }
            other => panic!("expected bedrock-invoke, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bedrock_invoke_without_region() {
        let json = r#"{
            "backends": [
                {
                    "kind": "bedrock-invoke",
                    "name": "x",
                    "region": "",
                    "model_id": "anthropic.claude-3-5-sonnet-20241022-v2:0"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("region")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bedrock_invoke_without_model_id() {
        let json = r#"{
            "backends": [
                {
                    "kind": "bedrock-invoke",
                    "name": "x",
                    "region": "us-east-1",
                    "model_id": ""
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("model_id")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn legacy_model_promotes_to_one_backend() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let resolved = cfg.resolved_backends();
        assert_eq!(resolved.len(), 1);
        match &resolved[0] {
            BackendEntry::Llamacpp(e) => {
                assert_eq!(e.name, "gemma-4-e4b");
                assert_eq!(e.n_ctx, 8192);
                assert_eq!(e.n_gpu_layers, 0);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
    }

    #[test]
    fn multi_backend_resolved_passes_through() {
        let f = write_config(&good_multi_backend_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let resolved = cfg.resolved_backends();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name(), "local-gemma");
        assert_eq!(resolved[1].name(), "anthropic-fallback");
    }

    #[test]
    fn listen_block_absent_by_default() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        assert!(cfg.listen.is_none());
    }

    #[test]
    fn listen_block_carries_tcp_and_api_key_env() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            },
            "listen": {
                "tcp": "127.0.0.1:9090",
                "tcp_v2": "127.0.0.1:9091",
                "api_key_env": "INFERD_TCP_API_KEY"
            }
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        let listen = cfg.listen.as_ref().expect("listen present");
        assert_eq!(listen.tcp.as_deref(), Some("127.0.0.1:9090"));
        assert_eq!(listen.tcp_v2.as_deref(), Some("127.0.0.1:9091"));
        assert_eq!(listen.api_key_env.as_deref(), Some("INFERD_TCP_API_KEY"));
    }

    #[test]
    fn llamacpp_entry_embed_defaults_off() {
        let f = write_config(&good_multi_backend_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let list = cfg.backends.as_ref().unwrap();
        match &list[0] {
            BackendEntry::Llamacpp(e) => {
                assert!(!e.embed);
                assert!(e.embed_pooling.is_none());
                assert_eq!(e.embed_n_ctx, 2048);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
    }

    #[test]
    fn llamacpp_entry_carries_embed_fields() {
        let json = r#"{
            "backends": [
                {
                    "kind": "llamacpp",
                    "name": "embeddings",
                    "embed": true,
                    "embed_pooling": 1,
                    "embed_n_ctx": 1024,
                    "model": {
                        "name": "embeddinggemma-300m",
                        "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                        "source_url": "https://example.com/embed.gguf"
                    }
                }
            ]
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        let list = cfg.backends.as_ref().unwrap();
        match &list[0] {
            BackendEntry::Llamacpp(e) => {
                assert!(e.embed);
                assert_eq!(e.embed_pooling, Some(1));
                assert_eq!(e.embed_n_ctx, 1024);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
    }

    #[test]
    fn legacy_promotion_keeps_embed_off() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let list = cfg.resolved_backends();
        match &list[0] {
            BackendEntry::Llamacpp(e) => {
                assert!(!e.embed);
                assert!(e.embed_pooling.is_none());
                assert_eq!(e.embed_n_ctx, 2048);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
    }

    #[test]
    fn listen_rejects_empty_tcp() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            },
            "listen": { "tcp": "   " }
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("listen.tcp")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn default_first_boot_config_has_generate_and_embed() {
        let cfg = default_first_boot_config();
        assert!(cfg.auto_pull, "first-boot default must auto-pull");
        let list = cfg.backends.as_ref().expect("backends present");
        assert_eq!(list.len(), 2, "default ships generate + embed");

        let mut saw_generate = false;
        let mut saw_embed = false;
        for entry in list {
            if let BackendEntry::Llamacpp(e) = entry {
                if e.embed {
                    saw_embed = true;
                    assert_eq!(e.model.name, "embeddinggemma-300m");
                } else {
                    saw_generate = true;
                    assert_eq!(e.model.name, "gemma-4-e4b");
                    // Multimodal by default (issue #30): the generate
                    // backend ships a vision projector so a fresh
                    // install has working v2 image attachments.
                    let mm = e
                        .mmproj
                        .as_ref()
                        .expect("default generate backend must carry an mmproj projector");
                    assert_eq!(mm.name, "gemma-4-e4b-mmproj");
                    assert!(
                        mm.source_url.ends_with("mmproj-F16.gguf"),
                        "default mmproj should be the F16 projector, got {}",
                        mm.source_url
                    );
                }
            }
        }
        assert!(saw_generate, "default must include a generate backend");
        assert!(saw_embed, "default must include an embed backend");
    }

    #[test]
    fn default_first_boot_config_validates() {
        // Round-trip the embedded default through the full
        // load+validate pipeline so we catch any drift between the
        // hard-coded literal and the validation rules.
        let cfg = default_first_boot_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let f = write_config(&json);
        ConfigFile::load(f.path()).expect("default config validates");
    }

    #[test]
    fn write_default_if_missing_writes_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert!(!path.exists());

        let wrote = write_default_if_missing(&path).unwrap();
        assert!(wrote, "should report wrote=true on first call");
        assert!(path.exists(), "default config now on disk");

        let cfg = ConfigFile::load(&path).unwrap();
        assert!(cfg.backends.is_some());
        assert!(cfg.auto_pull);
    }

    #[test]
    fn write_default_if_missing_does_not_overwrite() {
        // Operator customisation must win — the daemon never clobbers
        // an existing file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ \"i_am_user_data\": true }").unwrap();

        let wrote = write_default_if_missing(&path).unwrap();
        assert!(!wrote, "should report wrote=false when file exists");

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("i_am_user_data"),
            "operator file preserved verbatim"
        );
    }

    #[test]
    fn write_default_if_missing_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("subdir").join("config.json");
        assert!(!path.parent().unwrap().exists());

        let wrote = write_default_if_missing(&path).unwrap();
        assert!(wrote);
        assert!(path.exists());
    }
}
