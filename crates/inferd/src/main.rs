//! `inferd` — single CLI binary in the gh / kubectl shape.
//!
//! Distinct from `inferd-daemon` (the long-running service). The
//! `inferd` binary is what operators and consumers run from a
//! shell.
//!
//! v0.1 subcommand surface:
//!
//! - `inferd status`  — one-shot admin snapshot (the current
//!   lifecycle state) as JSON. Exits 0 on `ready`, non-zero
//!   otherwise. Useful for shell scripts.
//! - `inferd watch`   — stream admin events forever. Useful
//!   during the first-boot model download.
//! - `inferd pull`    — read `~/.inferd/config.json`, fetch
//!   the configured model into the CAS store
//!   (`$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`), verify SHA-
//!   256 with constant-time compare, write the manifest. Bypasses
//!   the daemon — operates directly on the store.
//! - `inferd doctor`  — diagnose connectivity. Prints a punch
//!   list of "what's there / what's missing" so consumers can
//!   debug install issues.
//!
//! Planned but not in v0.1: `inferd -p "hello world"` — connect
//! to the running daemon, send a one-shot prompt, stream tokens
//! to stdout. Replaces the previously-scaffolded `inferd-stdio`
//! crate (one binary, many shapes — gh / kubectl pattern).
//!
//! Out of scope for v0.1: `gc`, TCP API-key handling, per-
//! platform packaging concerns.

use clap::{Parser, Subcommand};
use inferd_client::AdminClient;
use inferd_daemon::admin::StatusBroadcaster;
use inferd_daemon::config_file::ConfigFile;
use inferd_daemon::fetch::{ModelSpec, fetch_model};
use inferd_daemon::status::StatusEvent;
use inferd_daemon::store::ModelStore;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "inferd",
    about = "Single CLI binary for inferd. Subcommands: status, watch, pull, doctor.",
    version,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Override config-file path. Defaults to `~/.inferd/config.json`.
    #[arg(long, env = "INFERD_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// Override admin-socket address. Defaults to the platform-
    /// specific path per `docs/protocol-v1.md` §"Admin endpoint".
    #[arg(long, env = "INFERD_ADMIN_ADDR", global = true)]
    admin_addr: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// One-shot admin snapshot. Prints the current lifecycle state
    /// (typically `ready`, `loading_model`, `starting`, etc.) as
    /// JSON. Exits 0 if `status: "ready"`, non-zero otherwise.
    Status,

    /// Stream admin events forever. Useful during first-boot model
    /// download to watch progress.
    Watch,

    /// Fetch the model named in the config into the shared CAS
    /// store. Bypasses the daemon — writes directly to
    /// `$MODELS_HOME/blobs/`. Idempotent: returns immediately if
    /// the manifest + blob already match.
    Pull,

    /// Diagnose connectivity. Prints a punch list of what's
    /// present and what's missing so consumers can debug install
    /// issues without grep'ing logs.
    Doctor,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    install_tracing();

    let cli = Cli::parse();
    let admin_addr = cli.admin_addr.clone().unwrap_or_else(default_admin_addr);
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(inferd_daemon::config_file::default_config_path);

    let result = match cli.command {
        Command::Status => cmd_status(&admin_addr).await,
        Command::Watch => cmd_watch(&admin_addr).await,
        Command::Pull => cmd_pull(&config_path).await,
        Command::Doctor => cmd_doctor(&admin_addr, &config_path).await,
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Plain stderr tracing; the CLI's stdout is for machine-readable
/// output (status JSON, watch events). Anything chatty goes to
/// stderr so `inferd status | jq` stays clean.
fn install_tracing() {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter =
        EnvFilter::try_from_env("INFERD_CLI_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .compact(),
        )
        .init();
}

/// Mirror of the daemon's resolution chain for the admin endpoint.
/// Kept in sync via the shared `endpoint::default_admin_addr`.
fn default_admin_addr() -> PathBuf {
    inferd_daemon::endpoint::default_admin_addr()
}

// --- status -----------------------------------------------------------

async fn cmd_status(admin_addr: &std::path::Path) -> anyhow::Result<ExitCode> {
    let mut admin = dial_admin(admin_addr).await?;
    let event = admin.recv().await?;
    println!("{}", admin_event_to_json(&event));
    let code = if event.status == "ready" {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    };
    Ok(code)
}

// --- watch ------------------------------------------------------------

async fn cmd_watch(admin_addr: &std::path::Path) -> anyhow::Result<ExitCode> {
    let mut admin = dial_admin(admin_addr).await?;
    loop {
        let event = admin.recv().await?;
        println!("{}", admin_event_to_json(&event));
        // Note: we don't exit on `draining` — the operator probably
        // wants to see the daemon coming back up. They Ctrl-C when
        // done.
    }
}

// --- pull -------------------------------------------------------------

async fn cmd_pull(config_path: &std::path::Path) -> anyhow::Result<ExitCode> {
    use anyhow::Context;

    let cfg = ConfigFile::load(config_path)
        .with_context(|| format!("loading config at {}", config_path.display()))?;

    // Open the store the daemon will use. Same resolution chain:
    // models_home in config > MODELS_HOME env > platform default.
    let store = match cfg.models_home.as_ref() {
        Some(p) => ModelStore::open(p),
        None => ModelStore::open(inferd_daemon::store::default_models_home()),
    };

    let spec: ModelSpec = (&cfg.model).into();
    eprintln!(
        "inferd: pulling {} -> {}",
        spec.name,
        store.root().display()
    );

    // The fetch_model function publishes progress through a
    // StatusBroadcaster; we don't have an admin socket here so we
    // just create a throwaway broadcaster. Status events are
    // dropped on the floor — for `inferd pull` the daemon's
    // stdout-style log lines from fetch.rs are enough.
    let broadcaster = StatusBroadcaster::new(StatusEvent::Starting);
    let bcast = std::sync::Arc::new(broadcaster);
    let spec_clone = spec.clone();
    let store_clone = store.clone();

    // fetch_model is sync (uses ureq blocking). Run on a blocking
    // thread so we don't block the runtime — though for a CLI we
    // could just call it directly. Keeping the boundary consistent
    // with how the daemon does it.
    let blob_path =
        tokio::task::spawn_blocking(move || fetch_model(&spec_clone, &store_clone, &bcast))
            .await
            .context("fetch task join")?
            .context("fetch failed")?;

    eprintln!("inferd: blob ready at {}", blob_path.display());
    Ok(ExitCode::SUCCESS)
}

// --- doctor -----------------------------------------------------------

async fn cmd_doctor(
    admin_addr: &std::path::Path,
    config_path: &std::path::Path,
) -> anyhow::Result<ExitCode> {
    let mut all_ok = true;
    let mut report_problem = |label: &str, ok: bool, detail: &str| {
        let mark = if ok { " ok " } else { "FAIL" };
        println!("[{mark}] {label}: {detail}");
        if !ok {
            all_ok = false;
        }
    };

    // 1. Config file present and parses?
    match ConfigFile::load(config_path) {
        Ok(cfg) => {
            report_problem(
                "config",
                true,
                &format!(
                    "loaded {} (model {}, auto_pull={})",
                    config_path.display(),
                    cfg.model.name,
                    cfg.auto_pull
                ),
            );

            // 2. Is the model on disk?
            let store = match cfg.models_home.as_ref() {
                Some(p) => ModelStore::open(p),
                None => ModelStore::open(inferd_daemon::store::default_models_home()),
            };
            let blob_path = store.blob_path(&cfg.model.sha256);
            if blob_path.exists() {
                let size = std::fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);
                report_problem(
                    "model",
                    true,
                    &format!("blob present ({} bytes) at {}", size, blob_path.display()),
                );
            } else {
                report_problem(
                    "model",
                    false,
                    &format!(
                        "blob missing at {}; run `inferd pull` or set auto_pull=true",
                        blob_path.display()
                    ),
                );
            }

            // 3. Manifest readable?
            match store.read_manifest(&cfg.model.name) {
                Ok(Some(_)) => {
                    report_problem(
                        "manifest",
                        true,
                        &format!(
                            "present at {}",
                            store.manifest_path(&cfg.model.name).display()
                        ),
                    );
                }
                Ok(None) => report_problem(
                    "manifest",
                    false,
                    &format!(
                        "missing at {}; daemon hasn't fetched yet, or operator skipped pull",
                        store.manifest_path(&cfg.model.name).display()
                    ),
                ),
                Err(e) => report_problem("manifest", false, &format!("read error: {e}")),
            }
        }
        Err(e) => {
            report_problem(
                "config",
                false,
                &format!("could not load {}: {e}", config_path.display()),
            );
        }
    }

    // 4. Admin socket reachable?
    match tokio::time::timeout(Duration::from_secs(1), dial_admin(admin_addr)).await {
        Ok(Ok(mut admin)) => {
            // Read one frame so we know the daemon's actual state.
            match tokio::time::timeout(Duration::from_secs(1), admin.recv()).await {
                Ok(Ok(event)) => report_problem(
                    "admin",
                    true,
                    &format!(
                        "connected at {}; daemon status={} phase={}",
                        admin_addr.display(),
                        event.status,
                        if event.phase.is_empty() {
                            "-"
                        } else {
                            &event.phase
                        }
                    ),
                ),
                _ => report_problem(
                    "admin",
                    false,
                    &format!(
                        "connected at {} but no frame within 1s",
                        admin_addr.display()
                    ),
                ),
            }
        }
        Ok(Err(e)) => report_problem(
            "admin",
            false,
            &format!(
                "{} not reachable: {e}; daemon not running, or wrong --admin-addr",
                admin_addr.display()
            ),
        ),
        Err(_) => report_problem(
            "admin",
            false,
            &format!("{} did not respond within 1s", admin_addr.display()),
        ),
    }

    if all_ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

// --- helpers ----------------------------------------------------------

/// Render an `AdminEvent` as one-line JSON, mirroring the wire
/// envelope the daemon publishes. We could just relay the raw
/// NDJSON line from the socket — but `AdminEvent` is the typed
/// view, and rebuilding the JSON from its fields keeps the CLI's
/// output schema-aligned with the spec rather than the daemon's
/// exact encoder choices (key order, whitespace).
fn admin_event_to_json(event: &inferd_client::AdminEvent) -> String {
    use serde_json::{Map, Value, json};
    let mut obj: Map<String, Value> = Map::new();
    obj.insert("id".into(), Value::String(event.id.clone()));
    obj.insert("type".into(), Value::String(event.kind.clone()));
    obj.insert("status".into(), Value::String(event.status.clone()));
    if !event.phase.is_empty() {
        obj.insert("phase".into(), Value::String(event.phase.clone()));
    }
    if let Some(p) = &event.path {
        obj.insert("path".into(), Value::String(p.clone()));
    }
    if let Some(d) = event.downloaded_bytes {
        obj.insert("downloaded_bytes".into(), json!(d));
    }
    if let Some(t) = event.total_bytes {
        obj.insert("total_bytes".into(), json!(t));
    }
    if let Some(u) = &event.source_url {
        obj.insert("source_url".into(), Value::String(u.clone()));
    }
    if let Some(s) = &event.expected_sha256 {
        obj.insert("expected_sha256".into(), Value::String(s.clone()));
    }
    if let Some(s) = &event.actual_sha256 {
        obj.insert("actual_sha256".into(), Value::String(s.clone()));
    }
    if let Some(p) = &event.quarantine_path {
        obj.insert("quarantine_path".into(), Value::String(p.clone()));
    }
    if let Some(n) = event.n_ctx {
        obj.insert("n_ctx".into(), json!(n));
    }
    serde_json::to_string(&Value::Object(obj)).unwrap_or_default()
}

#[cfg(unix)]
async fn dial_admin(path: &std::path::Path) -> anyhow::Result<AdminClient> {
    AdminClient::dial_admin_uds(path)
        .await
        .map_err(anyhow::Error::from)
}

#[cfg(windows)]
async fn dial_admin(path: &std::path::Path) -> anyhow::Result<AdminClient> {
    let s = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("admin pipe path is not valid UTF-8: {}", path.display()))?;
    AdminClient::dial_admin_pipe(s)
        .await
        .map_err(anyhow::Error::from)
}
