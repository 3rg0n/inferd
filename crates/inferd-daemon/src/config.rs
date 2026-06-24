//! Daemon CLI configuration.
//!
//! M1 keeps the CLI surface deliberately small: one transport choice
//! (`--tcp` or `--uds`), a lock path, a backend selector, and a queue
//! depth. The operator-flag matrix expands in M4 along with packaging.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Backend adapters the daemon can register at startup.
///
/// `LlamaCpp` is gated behind the `llamacpp` cargo feature — default
/// daemon builds only ship the mock adapter (per ADR 0006: lean core,
/// extensions are separate concerns). `OpenAiCompat` is gated behind
/// the `openai` cargo feature — pulled in only when the operator
/// wants the outbound HTTPS adapter (ADR 0006 cloud carve-out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    /// Deterministic test double — used by integration tests and the
    /// M1 echo daemon.
    Mock,
    /// Local llama.cpp backend via FFI (M2). Requires `--model-path`.
    #[cfg(feature = "llamacpp")]
    Llamacpp,
    /// OpenAI-compatible outbound HTTPS adapter (Phase 5A). Reaches
    /// any provider speaking the `/v1/chat/completions` wire (OpenAI,
    /// vLLM, LM Studio, LocalAI, OpenRouter, llama.cpp's HTTP server).
    /// Requires `--openai-base-url` + `--openai-model`. The API key
    /// is read from `--openai-api-key` or env (`INFERD_OPENAI_API_KEY`
    /// then `OPENAI_API_KEY`); pass an empty string to skip the
    /// `Authorization` header for self-hosted endpoints.
    #[cfg(feature = "openai")]
    OpenaiCompat,
    /// AWS Bedrock-runtime `InvokeModelWithResponseStream` adapter
    /// (Phase 6B-5). v0.2.0 ships only the Anthropic-on-Bedrock body
    /// shape — Claude models invoked via Bedrock's pinned
    /// `anthropic_version: "bedrock-2023-05-31"` payload. Requires
    /// `--bedrock-region` + `--bedrock-model-id`. Auth resolves from
    /// `--bedrock-bearer-token` / `AWS_BEARER_TOKEN_BEDROCK` first,
    /// then the standard `AWS_ACCESS_KEY_ID` /
    /// `AWS_SECRET_ACCESS_KEY` (+ optional `AWS_SESSION_TOKEN`)
    /// chain.
    #[cfg(feature = "bedrock")]
    BedrockInvoke,
}

/// Top-level CLI for `inferd-daemon`.
#[derive(Debug, Parser)]
#[command(name = "inferd-daemon", version, about = "Local inference daemon")]
pub struct Cli {
    /// Backend to load at startup.
    ///
    /// When omitted: defer to the config file's `backends:` (or legacy
    /// `model:` block) if one is present; otherwise fall back to the
    /// in-memory `mock` backend so `--lock + --tcp/--uds/--pipe` alone
    /// still boots a dev-mode echo daemon.
    ///
    /// When explicit: honour the CLI choice. Passing `--backend mock`
    /// short-circuits config loading (useful for forcing mock in test
    /// rigs even when a config file is on disk); any other explicit
    /// kind is built from CLI flags only — config-file `backends:` are
    /// ignored in that case so operators get exactly what they asked
    /// for.
    #[arg(long, value_enum, env = "INFERD_BACKEND")]
    pub backend: Option<BackendKind>,

    /// Path to the single-instance lock file. The lock is held for the
    /// lifetime of the daemon process.
    #[arg(long, env = "INFERD_LOCK")]
    pub lock: PathBuf,

    /// Unix domain socket path. Mutually exclusive with `--pipe`. Unix only.
    #[arg(long, env = "INFERD_UDS", conflicts_with_all = ["pipe"])]
    pub uds: Option<PathBuf>,

    /// Windows named pipe path (e.g. `\\.\pipe\inferd-infer`).
    /// Mutually exclusive with `--uds`. Windows only.
    #[arg(long, env = "INFERD_PIPE", conflicts_with_all = ["uds"])]
    pub pipe: Option<String>,

    /// Group name for the UDS (Unix only). Ignored on other transports.
    #[arg(long, env = "INFERD_GROUP")]
    pub group: Option<String>,

    /// Active generations served concurrently. v0.1 invariant is 1; values
    /// above 1 are reserved for v0.2 continuous-batching backends.
    #[arg(long, default_value_t = 1, env = "INFERD_ACTIVE_PERMITS")]
    pub active_permits: usize,

    /// Maximum waiting queue depth. Submits beyond this return
    /// `code: queue_full` immediately.
    #[arg(long, default_value_t = 10, env = "INFERD_QUEUE_DEPTH")]
    pub queue_depth: usize,

    /// Seconds to wait for the backend to report ready before failing
    /// startup.
    #[arg(long, default_value_t = 30, env = "INFERD_READY_TIMEOUT_SECS")]
    pub ready_timeout_secs: u64,

    /// Path to the GGUF model file. Required when `--backend llamacpp`.
    #[arg(long, env = "INFERD_MODEL_PATH")]
    pub model_path: Option<PathBuf>,

    /// Optional expected SHA-256 of the model file as a hex string
    /// (64 chars). When present, the daemon verifies the file before
    /// loading via `subtle::ConstantTimeEq` (THREAT_MODEL F-5).
    #[arg(long, env = "INFERD_MODEL_SHA256")]
    pub model_sha256: Option<String>,

    /// Llama.cpp context window in tokens. Default 8192.
    #[arg(long, default_value_t = 8192, env = "INFERD_N_CTX")]
    pub n_ctx: u32,

    /// Llama.cpp GPU layer offload count. 0 = CPU-only. GPU support
    /// requires the `cuda`/`metal`/`vulkan`/`rocm` cargo feature at
    /// build time.
    #[arg(long, default_value_t = 0, env = "INFERD_N_GPU_LAYERS")]
    pub n_gpu_layers: i32,

    /// Enable embed capability on the active llamacpp backend. Same
    /// flag plumbing as `--n-ctx` / `--n-gpu-layers`: when set, this
    /// CLI value flips `LlamacppEntry::embed = true` for both the
    /// legacy single-model promotion path (config has `model:`) AND
    /// the dev-mode path (no config file at all). When unset, the
    /// effective embed flag comes from the multi-backend
    /// `backends[].embed` field; legacy single-model and dev-mode
    /// stay generation-only.
    ///
    /// Mirrors the `--embed` flag's posture: this enables the
    /// *capability*; `--embed` separately decides whether to bind
    /// the embed socket. Both flags are needed to actually serve
    /// embed requests.
    #[arg(long, env = "INFERD_LLAMACPP_EMBED")]
    pub llamacpp_embed: bool,

    /// Pooling strategy for llamacpp embeddings. Maps to llama.cpp's
    /// `LLAMA_POOLING_TYPE_*`: 0 = NONE (per-token vectors), 1 = MEAN,
    /// 2 = CLS, 3 = LAST. When omitted, the model's metadata pooling
    /// default applies. Has no effect unless `--llamacpp-embed` is
    /// also set.
    #[arg(long, env = "INFERD_LLAMACPP_EMBED_POOLING")]
    pub llamacpp_embed_pooling: Option<i32>,

    /// Llama.cpp embed-side context window in tokens. Default 2048.
    /// Embed requests are bounded by this; generation is unaffected.
    /// Has no effect unless `--llamacpp-embed` is also set.
    #[arg(long, default_value_t = 2048, env = "INFERD_LLAMACPP_EMBED_N_CTX")]
    pub llamacpp_embed_n_ctx: u32,

    /// Base URL of the upstream OpenAI-compat endpoint, no trailing
    /// slash and no path (the adapter appends `/v1/chat/completions`).
    /// Required when `--backend openai-compat`. Examples:
    /// `https://api.openai.com`, `http://localhost:11434`,
    /// `https://openrouter.ai`.
    #[arg(long, env = "INFERD_OPENAI_BASE_URL")]
    pub openai_base_url: Option<String>,

    /// Bearer token for the OpenAI-compat upstream. Sent as
    /// `Authorization: Bearer <value>`. Pass an empty string to skip
    /// the header entirely for self-hosted endpoints. Resolves from
    /// `--openai-api-key`, then `INFERD_OPENAI_API_KEY`, then
    /// `OPENAI_API_KEY` (the de-facto env name most providers' SDKs
    /// already use).
    #[arg(long, env = "INFERD_OPENAI_API_KEY", hide_env_values = true)]
    pub openai_api_key: Option<String>,

    /// Upstream model identifier echoed in the request `model` field
    /// — provider-specific (e.g. `gpt-4o-mini`, `llama3.1:8b`,
    /// `meta-llama/Meta-Llama-3-70B-Instruct`). Required when
    /// `--backend openai-compat`.
    #[arg(long, env = "INFERD_OPENAI_MODEL")]
    pub openai_model: Option<String>,

    /// Total request timeout for OpenAI-compat calls, in seconds.
    /// Default 300 (5 minutes) — long enough for a slow first-token
    /// from a cold cloud model, short enough to surface stuck
    /// requests rather than hang forever.
    #[arg(long, default_value_t = 300, env = "INFERD_OPENAI_TIMEOUT_SECS")]
    pub openai_timeout_secs: u64,

    /// AWS region the Bedrock endpoint lives in, e.g. `us-east-1`,
    /// `eu-central-1`. Required when `--backend bedrock-invoke`.
    /// Used for both the endpoint host and SigV4 signing scope.
    #[arg(long, env = "INFERD_BEDROCK_REGION")]
    pub bedrock_region: Option<String>,

    /// Bedrock model id (URL-encoded by the adapter), e.g.
    /// `anthropic.claude-3-5-sonnet-20241022-v2:0`. Required when
    /// `--backend bedrock-invoke`.
    #[arg(long, env = "INFERD_BEDROCK_MODEL_ID")]
    pub bedrock_model_id: Option<String>,

    /// Pre-issued Bedrock bearer token (`AWS_BEARER_TOKEN_BEDROCK`
    /// shape, AWS rolled this out in 2025-06). When set, the adapter
    /// sends `Authorization: Bearer <value>` and skips SigV4. When
    /// unset, the adapter falls back to the standard
    /// `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (+ optional
    /// `AWS_SESSION_TOKEN`) chain via SigV4 signing.
    #[arg(long, env = "AWS_BEARER_TOKEN_BEDROCK", hide_env_values = true)]
    pub bedrock_bearer_token: Option<String>,

    /// Override the Bedrock endpoint host. Empty/absent → default
    /// `bedrock-runtime.<region>.amazonaws.com`. Useful for VPC
    /// endpoints / integration tests.
    #[arg(long, env = "INFERD_BEDROCK_ENDPOINT")]
    pub bedrock_endpoint: Option<String>,

    /// Total request timeout for Bedrock calls, in seconds. Default
    /// 300 (5 minutes).
    #[arg(long, default_value_t = 300, env = "INFERD_BEDROCK_TIMEOUT_SECS")]
    pub bedrock_timeout_secs: u64,

    /// Path to the operator JSON config file. Default
    /// `~/.inferd/config.json`. When present, fetch + auto-pull are
    /// driven from it; CLI flags (`--model-path`, `--model-sha256`,
    /// `--n-ctx`, `--n-gpu-layers`) override config-file values when
    /// both are supplied. When absent, the daemon falls back to
    /// CLI-flag-only operation (dev mode).
    #[arg(long, env = "INFERD_CONFIG")]
    pub config: Option<PathBuf>,

    /// Admin endpoint path. Defaults per-platform to the path
    /// documented in `docs/protocol-v1.md` §"Admin endpoint" — e.g.
    /// `/run/inferd/admin.sock` on Linux, `\\.\pipe\inferd-admin` on
    /// Windows. Override for tests / non-default deployments.
    #[arg(long, env = "INFERD_ADMIN_ADDR")]
    pub admin_addr: Option<PathBuf>,

    /// Enable the embed inference endpoint per ADR 0017. The embed
    /// endpoint binds on a socket separate from the generation socket:
    /// `infer.embed.sock` on Unix / `\\.\pipe\inferd-infer-embed`
    /// on Windows. Has no effect unless the active backend's
    /// `capabilities().embed` is true (capability-driven binding).
    #[arg(long, env = "INFERD_EMBED")]
    pub embed: bool,

    /// Override the default embed inference endpoint path.
    /// Mirrors `--uds` / `--pipe` for embed; on Linux/macOS this is
    /// a UDS path, on Windows a named-pipe path. Has no effect
    /// unless `--embed` is also set.
    #[arg(long, env = "INFERD_EMBED_ADDR")]
    pub embed_addr: Option<PathBuf>,
}

impl Cli {
    /// Validate that exactly one transport is selected. clap enforces
    /// mutual exclusion; this checks the at-least-one part.
    pub fn require_one_transport(&self) -> Result<(), &'static str> {
        let count = [self.uds.is_some(), self.pipe.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        match count {
            1 => Ok(()),
            0 => Err("must specify one of --uds, --pipe"),
            _ => Err("--uds and --pipe are mutually exclusive"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    #[cfg(unix)]
    fn cli_parses_minimum_required() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
        ]);
        assert!(cli.uds.is_some());
        assert!(cli.pipe.is_none());
        assert_eq!(cli.queue_depth, 10);
        assert_eq!(cli.active_permits, 1);
        cli.require_one_transport().unwrap();
    }

    #[test]
    fn cli_rejects_no_transport() {
        let cli = Cli::parse_from(["inferd-daemon", "--lock", "/tmp/inferd.lock"]);
        assert!(cli.require_one_transport().is_err());
    }

    #[test]
    #[cfg(unix)]
    fn cli_rejects_both_transports_via_clap() {
        // clap-level mutual exclusion: this should fail to parse, not
        // require_one_transport's runtime check.
        let result = Cli::try_parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--pipe",
            r"\\.\pipe\inferd-test",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_accepts_pipe_transport() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "C:/tmp/inferd.lock",
            "--pipe",
            r"\\.\pipe\inferd-test",
        ]);
        assert!(cli.pipe.is_some());
        assert!(cli.uds.is_none());
        cli.require_one_transport().unwrap();
    }

    #[test]
    fn cli_command_factory_is_well_formed() {
        // Ensures clap's `#[command]` derives don't conflict; cheap smoke
        // test that catches lots of misconfigurations.
        Cli::command().debug_assert();
    }

    #[test]
    #[cfg(unix)]
    fn cli_accepts_embed_flag() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--embed",
            "--embed-addr",
            "/tmp/inferd-embed.sock",
        ]);
        assert!(cli.embed);
        assert!(cli.embed_addr.is_some());
    }

    #[test]
    #[cfg(unix)]
    fn cli_embed_disabled_by_default() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
        ]);
        assert!(!cli.embed);
    }

    #[test]
    #[cfg(unix)]
    fn cli_llamacpp_embed_flags_default_off() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
        ]);
        assert!(!cli.llamacpp_embed);
        assert!(cli.llamacpp_embed_pooling.is_none());
        assert_eq!(cli.llamacpp_embed_n_ctx, 2048);
    }

    #[test]
    #[cfg(unix)]
    fn cli_accepts_llamacpp_embed_flags() {
        // Issue #16: dev-mode + legacy single-model configs need a
        // CLI route to flip embed on without rewriting the config.
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--llamacpp-embed",
            "--llamacpp-embed-pooling",
            "1",
            "--llamacpp-embed-n-ctx",
            "1024",
        ]);
        assert!(cli.llamacpp_embed);
        assert_eq!(cli.llamacpp_embed_pooling, Some(1));
        assert_eq!(cli.llamacpp_embed_n_ctx, 1024);
    }

    #[cfg(feature = "openai")]
    #[test]
    #[cfg(unix)]
    fn cli_accepts_openai_compat_backend() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--backend",
            "openai-compat",
            "--openai-base-url",
            "http://localhost:11434",
            "--openai-model",
            "llama3.1:8b",
            "--openai-api-key",
            "sk-x",
            "--openai-timeout-secs",
            "30",
        ]);
        assert_eq!(cli.backend, Some(BackendKind::OpenaiCompat));
        assert_eq!(
            cli.openai_base_url.as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(cli.openai_model.as_deref(), Some("llama3.1:8b"));
        assert_eq!(cli.openai_api_key.as_deref(), Some("sk-x"));
        assert_eq!(cli.openai_timeout_secs, 30);
    }

    #[cfg(feature = "bedrock")]
    #[test]
    #[cfg(unix)]
    fn cli_accepts_bedrock_invoke_backend() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--backend",
            "bedrock-invoke",
            "--bedrock-region",
            "us-east-1",
            "--bedrock-model-id",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "--bedrock-bearer-token",
            "abc123",
            "--bedrock-timeout-secs",
            "60",
        ]);
        assert_eq!(cli.backend, Some(BackendKind::BedrockInvoke));
        assert_eq!(cli.bedrock_region.as_deref(), Some("us-east-1"));
        assert_eq!(
            cli.bedrock_model_id.as_deref(),
            Some("anthropic.claude-3-5-sonnet-20241022-v2:0")
        );
        assert_eq!(cli.bedrock_bearer_token.as_deref(), Some("abc123"));
        assert_eq!(cli.bedrock_timeout_secs, 60);
    }

    #[cfg(feature = "bedrock")]
    #[test]
    #[cfg(unix)]
    fn cli_bedrock_timeout_defaults_to_300() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--backend",
            "bedrock-invoke",
            "--bedrock-region",
            "us-east-1",
            "--bedrock-model-id",
            "anthropic.claude-3-5-haiku-20241022-v1:0",
        ]);
        assert_eq!(cli.bedrock_timeout_secs, 300);
        assert!(cli.bedrock_bearer_token.is_none());
        assert!(cli.bedrock_endpoint.is_none());
    }

    #[cfg(feature = "openai")]
    #[test]
    #[cfg(unix)]
    fn cli_openai_timeout_defaults_to_300() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--uds",
            "/tmp/inferd.sock",
            "--backend",
            "openai-compat",
            "--openai-base-url",
            "https://api.openai.com",
            "--openai-model",
            "gpt-4o-mini",
        ]);
        assert_eq!(cli.openai_timeout_secs, 300);
        assert!(cli.openai_api_key.is_none());
    }
}
