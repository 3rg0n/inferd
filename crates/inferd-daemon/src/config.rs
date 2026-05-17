//! Daemon CLI configuration.
//!
//! M1 keeps the CLI surface deliberately small: one transport choice
//! (`--tcp` or `--uds`), a lock path, a backend selector, and a queue
//! depth. The operator-flag matrix expands in M4 along with packaging.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Backend adapters the daemon can register at startup. v0.1 ships only
/// the mock; M2 adds `llamacpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    /// Deterministic test double — used by integration tests and the
    /// M1 echo daemon.
    Mock,
    // Mock is the only one in v0.1. Llamacpp is M2.
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
}
