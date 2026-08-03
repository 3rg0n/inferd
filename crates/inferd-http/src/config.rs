//! CLI configuration for `inferd-http`.

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;

/// A daemon IPC endpoint — a UDS path on Unix, a named pipe on Windows.
/// Carried as a string; `dial_*` picks the platform-correct connect.
#[derive(Debug, Clone, Default)]
pub struct Endpoint(pub String);

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// OpenAI-compatible HTTP bridge for inferd. Point OpenCode (or any
/// OpenAI-SDK client) at this server's `/v1` and it talks to the local
/// inferd daemon.
#[derive(Debug, Parser)]
#[command(name = "inferd-http", version)]
pub struct Config {
    /// Address to bind. Defaults to loopback. Binding a non-loopback
    /// address requires `--token`.
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,

    /// Daemon generation endpoint (UDS path / pipe name). Defaults to
    /// the platform default the daemon binds.
    #[arg(long)]
    pub gen_addr_override: Option<String>,

    /// Daemon embeddings endpoint. Defaults to the platform default.
    #[arg(long)]
    pub embed_addr_override: Option<String>,

    /// Daemon admin endpoint. Defaults to the platform default. Used to
    /// read the backend's required audio sample rate (the daemon rejects
    /// any other rate and never resamples), so it is only dialed when a
    /// request actually carries `input_audio`.
    #[arg(long)]
    pub admin_addr_override: Option<String>,

    /// Bearer token required on inbound requests. Optional for loopback
    /// (the default is no auth); **required** to bind a non-loopback
    /// address.
    #[arg(long)]
    pub token: Option<String>,

    /// Model name advertised in responses and `/v1/models`. inferd
    /// serves one warm model; the bridge accepts any request `model`
    /// and echoes this back.
    #[arg(long, default_value = "inferd")]
    pub model_name: String,

    /// Seconds to wait for the daemon to be reachable+ready on each
    /// request's connect (retry-and-wait).
    #[arg(long, default_value_t = 30)]
    pub startup_timeout_secs: u64,

    // Resolved, non-CLI:
    #[arg(skip)]
    pub gen_addr: Endpoint,
    #[arg(skip)]
    pub embed_addr: Endpoint,
    #[arg(skip)]
    pub admin_addr: Endpoint,
    #[arg(skip)]
    pub startup_timeout: Duration,
}

impl Config {
    /// Parse args and resolve derived fields (endpoints, durations).
    pub fn parse() -> Self {
        let mut c = <Self as Parser>::parse();
        c.gen_addr = Endpoint(
            c.gen_addr_override
                .clone()
                .unwrap_or_else(default_gen_addr_string),
        );
        c.embed_addr = Endpoint(
            c.embed_addr_override
                .clone()
                .unwrap_or_else(default_embed_addr_string),
        );
        c.admin_addr = Endpoint(
            c.admin_addr_override
                .clone()
                .unwrap_or_else(default_admin_addr_string),
        );
        c.startup_timeout = Duration::from_secs(c.startup_timeout_secs);
        c
    }
}

fn default_gen_addr_string() -> String {
    inferd_client::default_v2_addr()
        .to_string_lossy()
        .into_owned()
}

fn default_embed_addr_string() -> String {
    inferd_client::default_embed_addr()
        .to_string_lossy()
        .into_owned()
}

fn default_admin_addr_string() -> String {
    inferd_client::default_admin_addr()
        .to_string_lossy()
        .into_owned()
}
