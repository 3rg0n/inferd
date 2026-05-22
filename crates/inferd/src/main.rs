//! `inferdctl` — single CLI binary in the gh / kubectl shape.
//!
//! Distinct from `inferd-daemon` (the long-running service). The
//! `inferdctl` binary is what operators and consumers run from a
//! shell.
//!
//! v0.1 subcommand surface:
//!
//! - `inferdctl status`  — one-shot admin snapshot (the current
//!   lifecycle state) as JSON. Exits 0 on `ready`, non-zero
//!   otherwise. Useful for shell scripts.
//! - `inferdctl watch`   — stream admin events forever. Useful
//!   during the first-boot model download.
//! - `inferdctl pull`    — read `~/.inferd/config.json`, fetch
//!   the configured model into the CAS store
//!   (`$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`), verify SHA-
//!   256 with constant-time compare, write the manifest. Bypasses
//!   the daemon — operates directly on the store.
//! - `inferdctl doctor`  — diagnose connectivity. Prints a punch
//!   list of "what's there / what's missing" so consumers can
//!   debug install issues.
//!
//! Planned but not in v0.1: `inferdctl -p "hello world"` — connect
//! to the running daemon, send a one-shot prompt, stream tokens
//! to stdout. Replaces the previously-scaffolded `inferd-stdio`
//! crate (one binary, many shapes — gh / kubectl pattern).
//!
//! Out of scope for v0.1: `gc`, TCP API-key handling, per-
//! platform packaging concerns.

use clap::{Parser, Subcommand};
use inferd_client::AdminClient;
use inferd_daemon::admin::StatusBroadcaster;
use inferd_daemon::config_file::{BackendEntry, ConfigFile, LlamacppEntry};
use inferd_daemon::fetch::{ModelSpec, fetch_model};
use inferd_daemon::status::StatusEvent;
use inferd_daemon::store::ModelStore;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "inferdctl",
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
/// stderr so `inferdctl status | jq` stays clean.
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

    // Multi-backend configs may declare several llamacpp entries
    // (each with its own model file) plus zero or more cloud
    // entries — only the local-model entries have a blob to pull.
    let llamacpp_entries: Vec<LlamacppEntry> = cfg
        .resolved_backends()
        .into_iter()
        .filter_map(|e| match e {
            BackendEntry::Llamacpp(l) => Some(l),
            _ => None,
        })
        .collect();

    if llamacpp_entries.is_empty() {
        eprintln!(
            "inferdctl: no llamacpp backends in {}; nothing to pull",
            config_path.display()
        );
        return Ok(ExitCode::SUCCESS);
    }

    for entry in &llamacpp_entries {
        let spec: ModelSpec = (&entry.model).into();
        eprintln!(
            "inferdctl: pulling {} -> {}",
            spec.name,
            store.root().display()
        );

        let broadcaster = StatusBroadcaster::new(StatusEvent::Starting);
        let bcast = std::sync::Arc::new(broadcaster);
        let spec_clone = spec.clone();
        let store_clone = store.clone();

        let blob_path =
            tokio::task::spawn_blocking(move || fetch_model(&spec_clone, &store_clone, &bcast))
                .await
                .context("fetch task join")?
                .context("fetch failed")?;

        eprintln!("inferdctl: blob ready at {}", blob_path.display());
    }
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
            let entries = cfg.resolved_backends();
            let llamacpp_entries: Vec<&LlamacppEntry> = entries
                .iter()
                .filter_map(|e| match e {
                    BackendEntry::Llamacpp(l) => Some(l),
                    _ => None,
                })
                .collect();
            let summary = entries
                .iter()
                .map(|e| match e {
                    BackendEntry::Llamacpp(l) => format!("llamacpp:{}", l.name),
                    BackendEntry::OpenaiCompat(o) => format!("openai-compat:{}", o.name),
                    BackendEntry::BedrockInvoke(b) => {
                        format!("bedrock-invoke:{}", b.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            report_problem(
                "config",
                true,
                &format!(
                    "loaded {} (backends: [{summary}], auto_pull={})",
                    config_path.display(),
                    cfg.auto_pull
                ),
            );

            // 2. Are the local models on disk? Only llamacpp entries
            // have a blob; cloud entries have nothing to check here
            // (their reachability is admin-socket / runtime concern).
            let store = match cfg.models_home.as_ref() {
                Some(p) => ModelStore::open(p),
                None => ModelStore::open(inferd_daemon::store::default_models_home()),
            };
            for entry in &llamacpp_entries {
                let label = format!("model[{}]", entry.name);
                let blob_path = store.blob_path(&entry.model.sha256);
                if blob_path.exists() {
                    let size = std::fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);
                    report_problem(
                        &label,
                        true,
                        &format!("blob present ({} bytes) at {}", size, blob_path.display()),
                    );
                } else {
                    report_problem(
                        &label,
                        false,
                        &format!(
                            "blob missing at {}; run `inferdctl pull` or set auto_pull=true",
                            blob_path.display()
                        ),
                    );
                }

                let manifest_label = format!("manifest[{}]", entry.name);
                match store.read_manifest(&entry.model.name) {
                    Ok(Some(_)) => {
                        report_problem(
                            &manifest_label,
                            true,
                            &format!(
                                "present at {}",
                                store.manifest_path(&entry.model.name).display()
                            ),
                        );
                    }
                    Ok(None) => report_problem(
                        &manifest_label,
                        false,
                        &format!(
                            "missing at {}; daemon hasn't fetched yet, or operator skipped pull",
                            store.manifest_path(&entry.model.name).display()
                        ),
                    ),
                    Err(e) => report_problem(&manifest_label, false, &format!("read error: {e}")),
                }
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
            // Read up to two frames: the daemon emits a capabilities
            // frame before the lifecycle snapshot when it's been
            // through backend construction. If we only see one frame,
            // it's the snapshot (daemon may not have hit Capabilities
            // yet — e.g. still in LoadingModel).
            let mut frames: Vec<inferd_client::AdminEvent> = Vec::new();
            for _ in 0..2 {
                match tokio::time::timeout(Duration::from_millis(500), admin.recv()).await {
                    Ok(Ok(event)) => frames.push(event),
                    _ => break,
                }
            }
            if frames.is_empty() {
                report_problem(
                    "admin",
                    false,
                    &format!(
                        "connected at {} but no frame within 1s",
                        admin_addr.display()
                    ),
                );
            } else {
                let caps = frames.iter().find(|e| e.status == "capabilities");
                let snapshot = frames
                    .iter()
                    .find(|e| e.status != "capabilities")
                    .unwrap_or(&frames[0]);
                report_problem(
                    "admin",
                    true,
                    &format!(
                        "connected at {}; daemon status={} phase={}",
                        admin_addr.display(),
                        snapshot.status,
                        if snapshot.phase.is_empty() {
                            "-"
                        } else {
                            &snapshot.phase
                        }
                    ),
                );
                if let Some(c) = caps {
                    let backend = c.backend.as_deref().unwrap_or("?");
                    let accel = c.accelerator.as_deref().unwrap_or("?");
                    let gpu_layers = c.gpu_layers.unwrap_or(0);
                    let v2 = c.v2.unwrap_or(false);
                    let vision = c.vision.unwrap_or(false);
                    let audio = c.audio.unwrap_or(false);
                    let tools = c.tools.unwrap_or(false);
                    let thinking = c.thinking.unwrap_or(false);
                    let embed = c.embed.unwrap_or(false);
                    report_problem(
                        "backend",
                        true,
                        &format!(
                            "{backend} accelerator={accel} gpu_layers={gpu_layers} \
                             v2={v2} vision={vision} audio={audio} tools={tools} \
                             thinking={thinking} embed={embed}"
                        ),
                    );
                }
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
    // capabilities frame (#77) — pass through every set field.
    if let Some(s) = &event.backend {
        obj.insert("backend".into(), Value::String(s.clone()));
    }
    if let Some(b) = event.v2 {
        obj.insert("v2".into(), json!(b));
    }
    if let Some(b) = event.vision {
        obj.insert("vision".into(), json!(b));
    }
    if let Some(b) = event.audio {
        obj.insert("audio".into(), json!(b));
    }
    if let Some(b) = event.tools {
        obj.insert("tools".into(), json!(b));
    }
    if let Some(b) = event.thinking {
        obj.insert("thinking".into(), json!(b));
    }
    if let Some(b) = event.embed {
        obj.insert("embed".into(), json!(b));
    }
    if let Some(s) = &event.accelerator {
        obj.insert("accelerator".into(), Value::String(s.clone()));
    }
    if let Some(n) = event.gpu_layers {
        obj.insert("gpu_layers".into(), json!(n));
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
