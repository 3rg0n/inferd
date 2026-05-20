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
/// extensions are separate concerns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    /// Deterministic test double — used by integration tests and the
    /// M1 echo daemon.
    Mock,
    /// Local llama.cpp backend via FFI (M2). Requires `--model-path`.
    #[cfg(feature = "llamacpp")]
    Llamacpp,
}

/// Top-level CLI for `inferd-daemon`.
#[derive(Debug, Parser)]
#[command(name = "inferd-daemon", version, about = "Local inference daemon")]
pub struct Cli {
    /// Backend to load at startup.
    #[arg(long, value_enum, default_value_t = BackendKind::Mock, env = "INFERD_BACKEND")]
    pub backend: BackendKind,

    /// Path to the single-instance lock file. The lock is held for the
    /// lifetime of the daemon process.
    #[arg(long, env = "INFERD_LOCK")]
    pub lock: PathBuf,

    /// Loopback TCP bind address. Mutually exclusive with `--uds` and `--pipe`.
    #[arg(long, env = "INFERD_TCP", conflicts_with_all = ["uds", "pipe"])]
    pub tcp: Option<String>,

    /// Unix domain socket path. Mutually exclusive with `--tcp` and `--pipe`. Unix only.
    #[arg(long, env = "INFERD_UDS", conflicts_with_all = ["tcp", "pipe"])]
    pub uds: Option<PathBuf>,

    /// Windows named pipe path (e.g. `\\.\pipe\inferd-infer`).
    /// Mutually exclusive with `--tcp` and `--uds`. Windows only.
    #[arg(long, env = "INFERD_PIPE", conflicts_with_all = ["tcp", "uds"])]
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

    /// Optional pre-shared API key. When set, TCP clients MUST send
    /// `{"type":"auth","key":"<this value>"}` as their first NDJSON
    /// frame on the connection or the daemon closes the connection.
    /// UDS and named-pipe transports ignore this — kernel-attested
    /// peer credentials (F-7) do the work there.
    ///
    /// Comparison is constant-time. THREAT_MODEL F-8.
    #[arg(long, env = "INFERD_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

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

    /// Enable the v2 inference endpoint per ADR 0015. v2 binds on a
    /// *separate* socket from v1: `infer.v2.sock` on Unix /
    /// `\\.\pipe\inferd-infer-v2` on Windows. v1 stays on its own
    /// socket and is unaffected.
    ///
    /// Phase 1B: the v2 endpoint accepts and validates v2 requests
    /// but returns `Error{code:internal, message:"v2 generation not
    /// implemented"}` because the Backend trait does not yet expose
    /// `generate_v2`. Use this to integration-test middleware that
    /// will speak v2 once Phase 2A lands.
    #[arg(long, env = "INFERD_V2")]
    pub v2: bool,

    /// Override the default v2 inference endpoint path.
    /// Mirrors `--uds` / `--pipe` for v2; on Linux/macOS this is a
    /// UDS path, on Windows a named-pipe path. Has no effect unless
    /// `--v2` is also set.
    #[arg(long, env = "INFERD_V2_ADDR")]
    pub v2_addr: Option<PathBuf>,

    /// Loopback TCP bind address for the v2 endpoint. Mutually
    /// exclusive with `--v2-addr`. Useful for tests that don't want
    /// the platform default (UDS / named pipe). Has no effect
    /// unless `--v2` is also set.
    #[arg(long, env = "INFERD_V2_TCP", conflicts_with = "v2_addr")]
    pub v2_tcp: Option<String>,
}

impl Cli {
    /// Validate that exactly one transport is selected. clap enforces
    /// mutual exclusion; this checks the at-least-one part.
    pub fn require_one_transport(&self) -> Result<(), &'static str> {
        let count = [self.tcp.is_some(), self.uds.is_some(), self.pipe.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        match count {
            1 => Ok(()),
            0 => Err("must specify one of --tcp, --uds, --pipe"),
            _ => Err("--tcp, --uds, --pipe are mutually exclusive"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_minimum_required() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--tcp",
            "127.0.0.1:0",
        ]);
        assert!(cli.tcp.is_some());
        assert!(cli.uds.is_none());
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
    fn cli_rejects_both_transports_via_clap() {
        // clap-level mutual exclusion: this should fail to parse, not
        // require_one_transport's runtime check.
        let result = Cli::try_parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--tcp",
            "127.0.0.1:0",
            "--uds",
            "/tmp/inferd.sock",
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
        assert!(cli.tcp.is_none());
        cli.require_one_transport().unwrap();
    }

    #[test]
    fn cli_rejects_pipe_with_tcp_via_clap() {
        let result = Cli::try_parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--tcp",
            "127.0.0.1:0",
            "--pipe",
            r"\\.\pipe\inferd-test",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_command_factory_is_well_formed() {
        // Ensures clap's `#[command]` derives don't conflict; cheap smoke
        // test that catches lots of misconfigurations.
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_accepts_v2_flag() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--tcp",
            "127.0.0.1:0",
            "--v2",
            "--v2-tcp",
            "127.0.0.1:0",
        ]);
        assert!(cli.v2);
        assert!(cli.v2_tcp.is_some());
        assert!(cli.v2_addr.is_none());
    }

    #[test]
    fn cli_rejects_v2_addr_with_v2_tcp() {
        let result = Cli::try_parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--tcp",
            "127.0.0.1:0",
            "--v2",
            "--v2-tcp",
            "127.0.0.1:0",
            "--v2-addr",
            "/tmp/inferd-v2.sock",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_v2_disabled_by_default() {
        let cli = Cli::parse_from([
            "inferd-daemon",
            "--lock",
            "/tmp/inferd.lock",
            "--tcp",
            "127.0.0.1:0",
        ]);
        assert!(!cli.v2);
    }
}
