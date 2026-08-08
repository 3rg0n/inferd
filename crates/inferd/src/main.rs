//! `inferdctl` — single CLI binary in the gh / kubectl shape.
//!
//! Distinct from `inferd-daemon` (the long-running service). The
//! `inferdctl` binary is what operators and consumers run from a
//! shell.
//!
//! Subcommand surface:
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
//! - `inferdctl import` — import a GGUF already on disk into the CAS
//!   store under a name, optionally checking it against an
//!   out-of-band `--expect-sha256`. The counterpart to `pull` for
//!   machines with no route to the internet (ADR 0028), and the only
//!   way bytes get in on the airgapped artifact.
//! - `inferdctl doctor`  — diagnose connectivity. Prints a punch
//!   list of "what's there / what's missing" so consumers can
//!   debug install issues.
//!
//! Planned for a future release: `inferdctl -p "hello world"` —
//! connect to the running daemon, send a one-shot prompt, stream
//! tokens to stdout (one binary, many shapes — gh / kubectl pattern).

use clap::{Parser, Subcommand};
use inferd_client::AdminClient;
use inferd_daemon::admin::StatusBroadcaster;
use inferd_daemon::config_file::{BackendEntry, ConfigFile, LlamacppEntry};
use inferd_daemon::fetch::{ModelSpec, fetch_model, import_model};
use inferd_daemon::status::StatusEvent;
use inferd_daemon::store::ModelStore;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Parser)]
// `long_version` carries the ADR 0028 build profile. `inferdctl` and the
// daemon ship in the same archive and are built with the same feature
// flags, so reusing the daemon's const means `inferdctl --version` is a
// truthful answer about the whole tarball — and it cannot drift, since
// the value is fixed by the same `cfg` that decides whether `ureq` is
// linked at all.
#[command(
    name = "inferdctl",
    about = "Single CLI binary for inferd. Subcommands: status, watch, pull, import, doctor.",
    version,
    long_version = inferd_daemon::LONG_VERSION,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Override config-file path. Defaults to `~/.inferd/config.json`.
    #[arg(long, env = "INFERD_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// Override admin-socket address. Defaults to the platform-
    /// specific path from `inferd_daemon::endpoint::default_admin_addr()`.
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

    /// Import a GGUF that is already on disk into the shared CAS
    /// store, then write a manifest so the daemon can resolve it by
    /// name. The offline counterpart to `pull` (ADR 0028) — on an
    /// airgapped build this is the only way bytes get in.
    ///
    /// The file is hashed as it is copied and left untouched; the
    /// store path is derived from the digest.
    Import {
        /// Model name to register, e.g. `gemma-4-e4b`. This is what
        /// `~/.inferd/config.json` refers to, and it becomes
        /// `manifests/<name>.json`.
        #[arg(long)]
        name: String,

        /// Expected SHA-256 as 64 lowercase hex characters. When
        /// given, the import is aborted (writing nothing) unless the
        /// file matches, using a constant-time compare. Supply the
        /// digest the model vendor published — without it the import
        /// still succeeds, it just has nothing to check against.
        #[arg(long, value_name = "HEX")]
        expect_sha256: Option<String>,

        /// Path to the GGUF file to import.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },

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
        Command::Import {
            ref name,
            ref expect_sha256,
            ref path,
        } => cmd_import(&config_path, name, expect_sha256.as_deref(), path).await,
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

/// Upper bound on admin frames `status` will read before giving up on
/// finding the lifecycle snapshot.
///
/// The daemon writes one capabilities frame per registered backend and
/// then exactly one snapshot frame (`admin::handle_admin_connection`), so
/// the snapshot is always at index `backends`. A daemon with more than
/// this many backends would be a very unusual deployment — ADR 0012 caps
/// it at one *model* per process — and the bound only exists so a
/// misbehaving or wedged peer cannot make `status` read forever.
const STATUS_MAX_FRAMES: usize = 64;

/// Read the daemon's on-connect burst: N capabilities frames followed by
/// one lifecycle snapshot.
///
/// Stops at the first non-`capabilities` frame, which is the snapshot.
/// Returns whatever arrived if the stream ends or stalls first, so a
/// daemon that is mid-load (no capabilities yet) and one that is wedged
/// both degrade to "report what we saw" rather than hanging.
///
/// Split out from [`cmd_status`] so the frame-selection logic is testable
/// without a socket: the bug this replaced (issue #57) was a fixed
/// two-frame read that no test could have caught, because nothing
/// exercised the loop.
async fn read_status_burst(admin: &mut AdminClient) -> Vec<inferd_client::AdminEvent> {
    let mut frames: Vec<inferd_client::AdminEvent> = Vec::new();
    for _ in 0..STATUS_MAX_FRAMES {
        match tokio::time::timeout(Duration::from_millis(500), admin.recv()).await {
            Ok(Ok(event)) => {
                let is_capabilities = event.status == "capabilities";
                frames.push(event);
                // The snapshot terminates the burst. Reading past it would
                // block for the full timeout waiting on the live event
                // stream, which for a healthy idle daemon never fires.
                if !is_capabilities {
                    break;
                }
            }
            _ => break,
        }
    }
    frames
}

async fn cmd_status(admin_addr: &std::path::Path) -> anyhow::Result<ExitCode> {
    let mut admin = dial_admin(admin_addr).await?;
    let frames = read_status_burst(&mut admin).await;
    if frames.is_empty() {
        anyhow::bail!("no admin frame received within 500ms");
    }
    for line in render_status(&frames) {
        println!("{line}");
    }
    if burst_reports_ready(&frames) {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

/// Lines `status` prints for a burst of admin frames: every capabilities
/// frame, then the lifecycle snapshot.
///
/// Printing *all* of them is the fix for the second half of issue #57.
/// The old code printed one frame and, on the shipped two-backend default
/// config, it was whichever backend sorted first — so an operator saw the
/// embed backend's `v2: false, vision: false` and read it as the daemon's
/// answer. Naming the generation backend instead would only move the
/// arbitrary pick: a config may register several generation backends
/// (ADR 0007 routes across them), and there is no basis for calling one
/// of them "the" backend. `doctor` already reports one line per backend
/// for exactly this reason; `status` now matches.
///
/// Frames are emitted in the order received, which is the daemon's
/// name-sorted capabilities order followed by the snapshot — stable
/// across runs, so `status | tail -1` is a usable idiom for the
/// lifecycle line.
fn render_status(frames: &[inferd_client::AdminEvent]) -> Vec<String> {
    frames.iter().map(admin_event_to_json).collect()
}

/// Whether a burst says the daemon is ready — the `status` exit code, as
/// a testable predicate (`ExitCode` is opaque, so asserting on it isn't).
///
/// Keyed on the lifecycle frame specifically. `capabilities` is not a
/// lifecycle state — it describes a backend, not the daemon — and since
/// `status` carries exactly one value, a capabilities frame can never
/// match `ready`. So a burst carrying only capabilities frames (a
/// truncated read, or a daemon that died mid-burst) is *not* ready and is
/// not reported as such, which is the half of issue #57 that made a
/// healthy daemon exit 1.
fn burst_reports_ready(frames: &[inferd_client::AdminEvent]) -> bool {
    frames.iter().any(|e| e.status == "ready")
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

// --- import -----------------------------------------------------------

/// Resolve the store `import` writes into.
///
/// Deliberately tolerant of a *missing* config file: on a fresh airgapped
/// machine the natural order is import-then-configure, and requiring a
/// config that names a model you have not imported yet would be a
/// chicken-and-egg. A config that *is* present is honoured for
/// `models_home`, so the import lands in the store the daemon will read.
///
/// Tolerant of `NotFound` **only**. A config that exists but is unreadable,
/// unparseable, or invalid is an error here for the same reason it is an
/// error in the daemon: it may carry a `models_home` we cannot read, and the
/// daemon rejects that same file loudly. Falling back to the platform default
/// would import into a store the daemon will never open, and report success.
fn store_for_import(config_path: &std::path::Path) -> anyhow::Result<ModelStore> {
    match ConfigFile::load(config_path) {
        Ok(cfg) => Ok(match cfg.models_home.as_ref() {
            Some(p) => ModelStore::open(p),
            None => ModelStore::open(inferd_daemon::store::default_models_home()),
        }),
        Err(inferd_daemon::config_file::ConfigError::NotFound(_)) => {
            // No config yet. MODELS_HOME / platform default is exactly
            // what the daemon falls back to as well.
            Ok(ModelStore::open(inferd_daemon::store::default_models_home()))
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "cannot determine the model store: config {} exists but is unusable \
             (fix it, or move it aside to import into the default store)",
            config_path.display()
        ))),
    }
}

/// `inferdctl import --name <n> [--expect-sha256 <hex>] <path.gguf>`.
///
/// The store it writes into is resolved by [`store_for_import`].
async fn cmd_import(
    config_path: &std::path::Path,
    name: &str,
    expect_sha256: Option<&str>,
    path: &std::path::Path,
) -> anyhow::Result<ExitCode> {
    use anyhow::Context;

    let store = store_for_import(config_path)?;

    eprintln!(
        "inferdctl: importing {} as {:?} -> {}",
        path.display(),
        name,
        store.root().display()
    );

    let src = path.to_path_buf();
    let name_owned = name.to_string();
    let expect_owned = expect_sha256.map(str::to_string);
    let store_clone = store.clone();
    let blob_path = tokio::task::spawn_blocking(move || {
        import_model(&src, &name_owned, expect_owned.as_deref(), &store_clone)
    })
    .await
    .context("import task join")?
    .context("import failed")?;

    println!("{}", blob_path.display());
    eprintln!(
        "inferdctl: imported; set \"model\": {{ \"name\": {name:?} }} in {} to use it",
        config_path.display()
    );
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
                // `manifest_path` rejects names that can't become a path
                // (ADR 0011 store layout). `read_manifest` would have
                // surfaced that as the `Err` arm below, so this is only
                // reached with a valid name — but doctor's job is to
                // report, never to panic on a bad config file.
                let manifest_where = store
                    .manifest_path(&entry.model.name)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|e| e.to_string());
                match store.read_manifest(&entry.model.name) {
                    Ok(Some(_)) => {
                        report_problem(
                            &manifest_label,
                            true,
                            &format!("present at {manifest_where}"),
                        );
                    }
                    Ok(None) => report_problem(
                        &manifest_label,
                        false,
                        &format!(
                            "missing at {manifest_where}; daemon hasn't fetched yet, \
                             or operator skipped pull"
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
            // The daemon writes one capabilities frame *per backend*
            // followed by the snapshot frame on connect (admin.rs). Drain
            // several frames so a multi-backend daemon's full capability
            // set is captured, not just the first — a 2-frame read would
            // miss a vision-capable generate backend behind an embed
            // backend (and vice versa).
            let mut frames: Vec<inferd_client::AdminEvent> = Vec::new();
            for _ in 0..8 {
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
                let caps_frames: Vec<&inferd_client::AdminEvent> = frames
                    .iter()
                    .filter(|e| e.status == "capabilities")
                    .collect();
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
                // One `backend:` line per registered backend, so a
                // multi-backend daemon reports e.g. a vision-capable
                // generate backend AND an embed backend, instead of
                // whichever frame happened to arrive first.
                for c in &caps_frames {
                    let backend = c.backend.as_deref().unwrap_or("?");
                    let accel = c.accelerator.as_deref().unwrap_or("?");
                    let gpu_layers = c.gpu_layers.unwrap_or(0);
                    let wire = c.wire_version.unwrap_or(0);
                    let v2 = c.v2.unwrap_or(false);
                    let vision = c.vision.unwrap_or(false);
                    let audio = c.audio.unwrap_or(false);
                    let tools = c.tools.unwrap_or(false);
                    let thinking = c.thinking.unwrap_or(false);
                    let embed = c.embed.unwrap_or(false);
                    let rerank = c.rerank.unwrap_or(false);
                    report_problem(
                        "backend",
                        true,
                        &format!(
                            "{backend} accelerator={accel} gpu_layers={gpu_layers} \
                             wire_version={wire} v2={v2} vision={vision} audio={audio} \
                             tools={tools} thinking={thinking} embed={embed} \
                             rerank={rerank}"
                        ),
                    );
                    if let Some(name) = c.device_name.as_deref() {
                        let vram = c
                            .vram_total_bytes
                            .map(format_bytes_short)
                            .unwrap_or_else(|| "?".to_string());
                        report_problem("device", true, &format!("{name} vram={vram}"));
                    }
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

/// Render a byte count as a short human string (e.g. `"24.0 GiB"`,
/// `"512 MiB"`). Used by `doctor` to print VRAM totals; the binary
/// (1024-based) form matches what GPU vendors quote.
fn format_bytes_short(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    const TIB: u64 = GIB * 1024;
    if n >= TIB {
        format!("{:.1} TiB", n as f64 / TIB as f64)
    } else if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{} MiB", n / MIB)
    } else if n >= KIB {
        format!("{} KiB", n / KIB)
    } else {
        format!("{n} B")
    }
}

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
    if let Some(b) = event.rerank {
        obj.insert("rerank".into(), json!(b));
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

#[cfg(test)]
mod tests {
    use super::store_for_import;
    use super::{STATUS_MAX_FRAMES, burst_reports_ready, read_status_burst, render_status};
    use inferd_client::{AdminClient, AdminEvent};
    use tokio::io::AsyncWriteExt;

    fn write(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let p = dir.join("config.json");
        std::fs::write(&p, body).unwrap();
        p
    }

    // Asserts the *fallback path is taken*, not what the default resolves
    // to: `default_models_home()` reads `MODELS_HOME`, so pinning a literal
    // path here would only assert the ambient environment.
    #[test]
    fn absent_config_falls_back_to_the_default_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-config.json");

        let store =
            store_for_import(&path).expect("a missing config is the import-then-configure case");
        assert_eq!(
            store.root(),
            inferd_daemon::store::default_models_home().as_path()
        );
    }

    #[test]
    fn present_config_is_honoured_for_models_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("store");
        let path = write(
            dir.path(),
            &serde_json::json!({
                "models_home": home.to_string_lossy(),
                "backends": [{
                    "kind": "llamacpp",
                    "name": "gen",
                    "model": { "name": "m", "sha256": "0".repeat(64), "source_url": "" }
                }]
            })
            .to_string(),
        );

        let store = store_for_import(&path).expect("a valid config must be accepted");
        assert_eq!(store.root(), home.as_path());
    }

    /// The regression: an *invalid* config used to be swallowed, and the
    /// import silently landed in the platform-default store — a store the
    /// daemon, which rejects the same file loudly, will never open.
    #[test]
    fn invalid_config_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("store");
        // Valid JSON, fails validation: an empty backends list without
        // `model_autoselect: "auto"`.
        let path = write(
            dir.path(),
            &serde_json::json!({ "models_home": home.to_string_lossy(), "backends": [] })
                .to_string(),
        );

        let err = store_for_import(&path).expect_err("an invalid config must not be swallowed");
        let text = format!("{err:#}");
        assert!(
            text.contains("cannot determine the model store"),
            "unexpected error: {text}"
        );
        // The operator must be told *what* is wrong, not just that
        // something is: `main` prints the anyhow chain with `{e:#}`.
        assert!(
            text.contains("backends list must not be empty"),
            "the underlying config error must survive into the message: {text}"
        );
    }

    #[test]
    fn unparseable_config_is_an_error_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "{ not json");

        let err = store_for_import(&path).expect_err("a corrupt config must not be swallowed");
        let text = format!("{err:#}");
        assert!(text.contains("cannot determine the model store"), "{text}");
        assert!(text.contains("parse"), "{text}");
    }

    // --- status: the #57 burst read -----------------------------------

    fn caps(backend: &str) -> String {
        serde_json::json!({
            "id": "admin", "type": "status", "status": "capabilities",
            "backend": backend, "v2": backend.starts_with("gemma"),
        })
        .to_string()
    }

    fn lifecycle(status: &str) -> String {
        serde_json::json!({ "id": "admin", "type": "status", "status": status }).to_string()
    }

    fn event(line: &str) -> AdminEvent {
        serde_json::from_str(line).expect("test fixture must be a valid admin frame")
    }

    /// An admin subscriber fed a canned NDJSON burst.
    ///
    /// `hold` keeps the writer half alive, which is what a healthy idle
    /// daemon looks like on the wire: no EOF, just nothing further to say
    /// until something happens. Dropping it instead gives the closed-socket
    /// case. The distinction is the whole reason the burst read needs both
    /// a terminator and a timeout.
    async fn canned_admin(
        lines: &[String],
        hold: bool,
    ) -> (AdminClient, Option<tokio::io::DuplexStream>) {
        let (mut server, client) = tokio::io::duplex(256 * 1024);
        for line in lines {
            server.write_all(line.as_bytes()).await.unwrap();
            server.write_all(b"\n").await.unwrap();
        }
        let keep = if hold { Some(server) } else { None };
        (AdminClient::wrap_for_test(Box::new(client)), keep)
    }

    /// Issue #57. The daemon replays one capabilities frame per registered
    /// backend before the lifecycle snapshot, so on the shipped two-backend
    /// default the old fixed `0..2` read consumed both capabilities frames
    /// and never saw `ready` — a healthy daemon reported exit 1 and printed
    /// whichever backend sorted first.
    #[tokio::test]
    async fn status_reads_past_every_capabilities_frame() {
        let burst = [
            caps("embeddinggemma-300m"),
            caps("gemma-4-e4b"),
            lifecycle("ready"),
        ];
        let (mut admin, _hold) = canned_admin(&burst, false).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(frames.len(), 3, "all three frames must be read: {frames:?}");
        assert!(burst_reports_ready(&frames), "a ready daemon must exit 0");

        // Both backends are reported, not an arbitrary one of them.
        let lines = render_status(&frames);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("embeddinggemma-300m"), "{lines:?}");
        assert!(lines[1].contains("gemma-4-e4b"), "{lines:?}");
        assert!(lines[2].contains(r#""status":"ready""#), "{lines:?}");
        for line in &lines {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("status must print valid JSON, got {line}: {e}"));
        }
    }

    /// The single-backend config the old code did handle. Latent bug, so
    /// this is the case that kept #57 from being caught earlier.
    #[tokio::test]
    async fn single_backend_burst_is_ready() {
        let burst = [caps("gemma-4-e4b"), lifecycle("ready")];
        let (mut admin, _hold) = canned_admin(&burst, false).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(frames.len(), 2);
        assert!(burst_reports_ready(&frames));
    }

    /// `capabilities` describes a backend, not the daemon. A burst that
    /// carries only capabilities frames — daemon died mid-burst, or the
    /// read was truncated — must not be reported as ready.
    #[tokio::test]
    async fn capabilities_only_burst_is_not_ready() {
        let burst = [caps("gemma-4-e4b"), caps("embeddinggemma-300m")];
        let (mut admin, _hold) = canned_admin(&burst, false).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(frames.len(), 2);
        assert!(
            !burst_reports_ready(&frames),
            "no lifecycle frame arrived, so the daemon is not known-ready"
        );
    }

    /// A pre-`ready` lifecycle state terminates the burst just as `ready`
    /// does, and exits non-zero: the daemon is up but not serving.
    #[tokio::test]
    async fn loading_snapshot_terminates_the_burst_and_is_not_ready() {
        let burst = [caps("gemma-4-e4b"), lifecycle("loading_model")];
        let (mut admin, _hold) = canned_admin(&burst, false).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].status, "loading_model");
        assert!(!burst_reports_ready(&frames));
    }

    /// The snapshot ends the read. Anything after it belongs to the live
    /// event stream, which is `watch`'s job — `status` must not consume it,
    /// and must not sit waiting for it either.
    #[tokio::test]
    async fn burst_stops_at_the_snapshot() {
        let burst = [
            caps("gemma-4-e4b"),
            lifecycle("ready"),
            lifecycle("draining"),
        ];
        let (mut admin, _hold) = canned_admin(&burst, true).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(
            frames.len(),
            2,
            "the frame after the snapshot is a live event: {frames:?}"
        );
    }

    /// The read is bounded, so a peer that streams capabilities frames
    /// forever cannot make `status` loop without end.
    #[tokio::test]
    async fn read_is_bounded_by_status_max_frames() {
        let burst: Vec<String> = (0..STATUS_MAX_FRAMES + 8)
            .map(|i| caps(&format!("backend-{i}")))
            .collect();
        let (mut admin, _hold) = canned_admin(&burst, true).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(frames.len(), STATUS_MAX_FRAMES);
        assert!(!burst_reports_ready(&frames));
    }

    /// A daemon that sends some of the burst and then goes quiet without
    /// closing: report what arrived rather than hanging. Paused clock, so
    /// the timeout fires without a real half-second wait.
    #[tokio::test(start_paused = true)]
    async fn stalled_daemon_degrades_to_what_arrived() {
        let burst = [caps("gemma-4-e4b")];
        let (mut admin, _hold) = canned_admin(&burst, true).await;

        let frames = read_status_burst(&mut admin).await;

        assert_eq!(frames.len(), 1);
        assert!(!burst_reports_ready(&frames));
    }

    /// `cmd_status` bails on an empty burst rather than printing nothing
    /// and exiting 1, which would be indistinguishable from a real
    /// not-ready answer.
    #[tokio::test]
    async fn immediately_closed_socket_yields_no_frames() {
        let (mut admin, _hold) = canned_admin(&[], false).await;

        assert!(read_status_burst(&mut admin).await.is_empty());
    }

    /// Ordering is the daemon's, not ours: capabilities in the order sent,
    /// then the snapshot last. That is what makes `status | tail -1` a
    /// usable idiom for the lifecycle line.
    #[test]
    fn render_preserves_frame_order() {
        let frames = [
            event(&caps("a-backend")),
            event(&caps("z-backend")),
            event(&lifecycle("ready")),
        ];
        let lines = render_status(&frames);
        assert!(lines[0].contains("a-backend"));
        assert!(lines[1].contains("z-backend"));
        assert!(lines[2].contains(r#""status":"ready""#));
    }
}
