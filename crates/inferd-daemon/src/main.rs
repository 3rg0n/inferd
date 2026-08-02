//! `inferd-daemon` binary entrypoint.
//!
//! Bring-up order (per `docs/protocol-v1.md` §"Admin endpoint"):
//!
//! 1. Initialise tracing.
//! 2. Acquire single-instance lock (THREAT_MODEL F-2).
//! 3. Build `StatusBroadcaster` seeded with `Starting`; bind admin
//!    socket *immediately* so installer GUIs and middleware can
//!    connect during the rest of bring-up.
//! 4. Read `~/.inferd/config.json` if present; otherwise fall back
//!    to CLI-only operation (dev mode against `--backend mock`).
//! 5. Resolve model: fetch if missing AND `auto_pull`; verify SHA;
//!    publish `loading_model` events through the broadcaster as
//!    each phase progresses.
//! 6. Construct backend (`Mock` for dev, `LlamaCpp` for production);
//!    publish `mmap` + `kv_cache` phase events.
//! 7. Build router; wait for `Backend::ready()`.
//! 8. Publish `Ready`.
//! 9. Bind inference socket (THREAT_MODEL F-13: not before ready).
//! 10. Serve until shutdown signal.

use clap::Parser;
use inferd_daemon::admin::StatusBroadcaster;
use inferd_daemon::config::{BackendKind, Cli};
#[cfg(any(feature = "llamacpp", feature = "openai", feature = "bedrock"))]
use inferd_daemon::config_file::BackendEntry;
#[cfg(feature = "bedrock")]
use inferd_daemon::config_file::BedrockInvokeEntry;
use inferd_daemon::config_file::ConfigFile;
#[cfg(feature = "llamacpp")]
use inferd_daemon::config_file::LlamacppEntry;
#[cfg(feature = "openai")]
use inferd_daemon::config_file::OpenaiCompatEntry;
#[cfg(unix)]
use inferd_daemon::endpoint::bind_admin_uds;
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
use inferd_daemon::endpoint::default_admin_addr;
use inferd_daemon::lifecycle::{AcceptContext, wait_for_ready};
use inferd_daemon::lock::Lock;
use inferd_daemon::logx::{DEFAULT_ROTATE_BYTES, LogxLayer, LogxWriter, default_log_dir};
use inferd_daemon::router::Router;
use inferd_daemon::status::{LoadPhase, StatusEvent};
use inferd_engine::{Backend, mock::Mock};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    install_tracing()?;

    // Windows: when launched from the per-user Startup shortcut (or a
    // double-click), a console-subsystem exe gets a fresh console window
    // allocated for it, which sits visibly on the desktop showing tracing
    // output (issue #28). Detach from it now that logging is wired to the
    // activity log + admin pipe. Only detaches when we OWN the console
    // (sole attached process); a daemon launched from an interactive shell
    // shares that shell's console and is left alone so `inferd-daemon` still
    // prints when run by hand for debugging.
    #[cfg(windows)]
    detach_own_console();

    let cli = Cli::parse();
    // Note: at-least-one-transport check is deferred until after the
    // config file is loaded — the operator may declare TCP via
    // `listen.tcp` in config.json instead of `--tcp` on the CLI
    // (Phase 6B-4). clap still enforces mutual exclusion when CLI
    // flags ARE set.

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "inferd-daemon starting"
    );

    // Ensure the runtime directory exists before touching any path under it.
    // On macOS, $TMPDIR/inferd/ is not pre-created by launchd; on Linux,
    // RuntimeDirectory= in the systemd unit handles this. We create it here
    // so the daemon self-heals on both platforms regardless of how it was
    // started.
    if let Some(parent) = cli.lock.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("create runtime dir {:?}: {e}", parent))?;
    }

    // Lock first — single-instance invariant (THREAT_MODEL F-2).
    let _lock =
        Lock::acquire(&cli.lock).map_err(|e| anyhow::anyhow!("lock acquire failed: {e}"))?;
    info!(path = %cli.lock.display(), "single-instance lock held");

    // Status broadcaster, seeded with Starting. Bound to the admin
    // socket immediately — clients can connect from now until the
    // daemon exits.
    let broadcaster = Arc::new(StatusBroadcaster::new(StatusEvent::Starting));
    let (admin_shutdown_tx, admin_handle) =
        spawn_admin_listener(Arc::clone(&broadcaster), cli.admin_addr.clone()).await?;

    // Try to load the operator config file. Dev mode (no config) is
    // permitted: the daemon runs CLI-only against the mock backend.
    let config = load_config_file(cli.config.as_deref());

    // ADR 0023: boot-time model auto-selection. When the config opts in
    // (`model_autoselect: "auto"`) and has not pinned a generation
    // backend, pick the Gemma 4 variant by the chosen accelerator's
    // total memory and rewrite the (in-memory) backend list. No-op
    // otherwise. Requires the dl-backends runtime accelerator probe;
    // without it there is no memory query, so auto-select degrades to
    // E4B (the safe default) via `apply(None, None)`.
    let config = apply_model_autoselect(config);

    // Resolve the generation transport (ADR 0021: one generation
    // socket). CLI flags win; else the platform-default socket/pipe
    // so a stock install needs no flag. TCP is no longer supported
    // per ADR 0022.
    let resolved_gen = resolve_transport(&cli, config.as_ref())?;

    // Resolve models + construct backends. Publishes loading_model
    // phase events through the broadcaster. Returns the canonical
    // ordered list (multi-backend per ADR 0007).
    let (backends, backend_labels): (Vec<Arc<dyn Backend>>, Vec<String>) =
        match build_backends(&cli, config.as_ref(), Arc::clone(&broadcaster)).await {
            Ok(b) => b,
            Err(e) => {
                // Log to the NDJSON activity log before shutting down so
                // `inferdctl doctor` and log tailing see the real cause.
                // In background-service mode (systemd/launchd) stderr may
                // not be visible; this is the only diagnostic surface that
                // survives a failed start (issue #47).
                error!(error = %e, "backend init failed — daemon is shutting down");
                let _ = admin_shutdown_tx.send(());
                let _ = tokio::time::timeout(Duration::from_secs(1), admin_handle).await;
                return Err(e);
            }
        };
    for b in &backends {
        info!(name = b.name(), "backend constructed");
    }

    // Publish capability snapshot so admin subscribers can introspect
    // multimodal / tools / accelerator posture before Ready (#77).
    // One frame per backend so subscribers see the full router shape.
    for (b, label) in backends.iter().zip(backend_labels.iter()) {
        let caps = b.capabilities();
        broadcaster.publish(StatusEvent::Capabilities {
            // Unique config-entry label (e.g. "gemma-4-e4b"), not
            // `b.name()` which is the kind ("llamacpp") and collides
            // across entries — that collision made the caps map drop all
            // but the last backend (doctor's vision=false bug).
            backend: label.clone(),
            wire_version: inferd_proto::v2::WIRE_VERSION,
            v2: caps.v2,
            vision: caps.vision,
            audio: caps.audio,
            tools: caps.tools,
            thinking: caps.thinking,
            embed: caps.embed,
            accelerator: caps.accelerator.kind.as_str().to_string(),
            gpu_layers: caps.accelerator.gpu_layers,
            device_name: caps.accelerator.device_name.clone(),
            vram_total_bytes: caps.accelerator.vram_total_bytes,
        });
    }

    // Decide whether to bind the embed socket: opt-in via `--embed`
    // AND at least one registered backend advertises `embed`
    // capability (ADR 0017 §"Capability-driven binding"). Without
    // both, the embed socket simply isn't bound.
    let embed_enabled = cli.embed && backends.iter().any(|b| b.capabilities().embed);
    if cli.embed && !embed_enabled {
        warn!(
            "--embed requested but no registered backend advertises \
             `capabilities().embed = true`; embed socket will not bind"
        );
    }

    // Build router. Walks the ordered list per ADR 0007.
    let router = Arc::new(Router::new(backends));

    // Wait for Backend::ready (F-13). Mock flips ready immediately;
    // LlamaCpp flips ready in `new()` after model load + KV cache.
    let waited = wait_for_ready(&router, Duration::from_secs(cli.ready_timeout_secs))
        .await
        .map_err(|e| anyhow::anyhow!("backend ready: {e}"))?;
    info!(?waited, "backend ready");

    // Publish Ready BEFORE binding the inference socket. Admin
    // subscribers see the ready frame; THEN inference clients can
    // connect — guaranteed ordering on the wire.
    broadcaster.publish(StatusEvent::Ready);

    // Inference shutdown channels — one for the generation listener,
    // plus the embed listener when enabled + capability matches.
    let fanout = 1 + usize::from(embed_enabled);
    let mut shutdown_rxs = install_shutdown_signal(fanout)?;
    let inference_shutdown_tx = shutdown_rxs.remove(0);
    let embed_shutdown_tx = if embed_enabled {
        Some(shutdown_rxs.remove(0))
    } else {
        None
    };

    let admission = inferd_daemon::queue::Admission::new(cli.active_permits, cli.queue_depth);
    info!(
        active_permits = cli.active_permits,
        queue_depth = cli.queue_depth,
        capacity = admission.capacity(),
        "admission gate configured"
    );

    // 0 disables the bound (operator escape hatch); anything else is a
    // hard ceiling on one response write (THREAT_MODEL F-17).
    let write_timeout = (cli.write_timeout_secs > 0).then(|| {
        let d = Duration::from_secs(cli.write_timeout_secs);
        info!(write_timeout = ?d, "response write timeout configured");
        d
    });
    if write_timeout.is_none() {
        warn!(
            "response write timeout disabled (--write-timeout-secs 0): a peer that stops \
             reading can hold an admission slot indefinitely"
        );
    }

    let accept_ctx = AcceptContext {
        admission: Some(admission),
        write_timeout,
    };

    // (v0.4 / ADR 0021) There is no separate v2 listener anymore — the
    // single generation listener below serves the v2 framing. The embed
    // listener still runs in parallel on its own socket.
    let v2_handle: Option<tokio::task::JoinHandle<std::io::Result<()>>> = { None };

    // Spawn the embed listener if enabled and the active backend
    // advertises embed capability (ADR 0017). Embed shares the same
    // Router + admission gate as v1 / v2.
    let embed_handle = if let Some(rx) = embed_shutdown_tx {
        Some(spawn_embed_listener(&cli, Arc::clone(&router), accept_ctx.clone(), rx).await?)
    } else {
        None
    };

    // The single generation listener serves the v2 framing (ADR 0021).
    let serve_result = match resolved_gen {
        #[cfg(unix)]
        ResolvedTransport::Uds(path) => {
            let listener = bind_uds(&path, cli.group.as_deref()).await?;
            info!(path = %path.display(), "generation uds listener bound");
            inferd_daemon::lifecycle_v2::serve_uds_v2(
                listener,
                router,
                accept_ctx,
                inference_shutdown_tx,
            )
            .await
        }
        #[cfg(not(unix))]
        ResolvedTransport::Uds(path) => {
            drop((path, router, accept_ctx, inference_shutdown_tx));
            anyhow::bail!("Unix domain sockets are not supported on this platform; use --pipe");
        }
        #[cfg(windows)]
        ResolvedTransport::Pipe(path) => {
            let first = inferd_daemon::endpoint::bind_named_pipe(&path, true)?;
            info!(path = %path, "generation named pipe listener bound");
            inferd_daemon::lifecycle_v2::serve_named_pipe_v2(
                &path,
                first,
                router,
                accept_ctx,
                inference_shutdown_tx,
            )
            .await
        }
        #[cfg(not(windows))]
        ResolvedTransport::Pipe(path) => {
            drop((path, router, accept_ctx, inference_shutdown_tx));
            anyhow::bail!("Windows named pipes are not supported on this platform; use --uds");
        }
    };

    // Drain: tell admin subscribers we're going away, then close.
    broadcaster.publish(StatusEvent::Draining);
    let _ = admin_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), admin_handle).await;

    // Wait for the v2 listener to finish draining if it was running.
    if let Some(handle) = v2_handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    // Same for embed.
    if let Some(handle) = embed_handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    serve_result?;
    info!("shutdown complete");
    Ok(())
}

// (v0.4 / ADR 0021) `spawn_v2_listener` was removed: v2 is no longer a
// separate opt-in listener — the single generation listener in `main`
// serves the v2 framing on the default/configured transport.

/// Bind the embed inference listener and spawn its accept loop.
/// Returns the JoinHandle so main can await graceful drain. Embed is
/// per ADR 0017: separate socket from v1/v2; reuses the same admission
/// gate + Router. Bound only when the active router has at least one
/// backend with `BackendCapabilities::embed == true`.
async fn spawn_embed_listener(
    cli: &Cli,
    router: Arc<Router>,
    accept_ctx: AcceptContext,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    use inferd_daemon::endpoint::default_embed_addr;
    use inferd_daemon::lifecycle_embed;

    let path = cli.embed_addr.clone().unwrap_or_else(default_embed_addr);
    #[cfg(unix)]
    {
        let listener = bind_uds(&path, cli.group.as_deref()).await?;
        info!(path = %path.display(), "embed uds listener bound");
        Ok(tokio::spawn(async move {
            if let Err(e) =
                lifecycle_embed::serve_uds_embed(listener, router, accept_ctx, shutdown_rx).await
            {
                error!(error = ?e, "embed uds listener error");
            }
        }))
    }
    #[cfg(windows)]
    {
        let path_str = path
            .to_str()
            .ok_or_else(|| {
                anyhow::anyhow!("embed pipe path is not valid utf-8: {}", path.display())
            })?
            .to_string();
        let first = inferd_daemon::endpoint::bind_named_pipe(&path_str, true)?;
        info!(path = %path_str, "embed named pipe listener bound");
        Ok(tokio::spawn(async move {
            if let Err(e) = lifecycle_embed::serve_named_pipe_embed(
                &path_str,
                first,
                router,
                accept_ctx,
                shutdown_rx,
            )
            .await
            {
                error!(error = ?e, "embed named pipe listener error");
            }
        }))
    }
    #[cfg(not(any(unix, windows)))]
    {
        drop((path, router, accept_ctx, shutdown_rx));
        anyhow::bail!("embed endpoint requires unix or windows")
    }
}

/// Bind the admin socket and spawn the accept loop. Returns the
/// shutdown sender + the join handle so main can clean up on exit.
async fn spawn_admin_listener(
    broadcaster: Arc<StatusBroadcaster>,
    cli_addr: Option<PathBuf>,
) -> anyhow::Result<(
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
)> {
    let addr = cli_addr.unwrap_or_else(default_admin_addr);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    #[cfg(unix)]
    {
        let listener = bind_admin_uds(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("bind admin socket {}: {e}", addr.display()))?;
        info!(path = %addr.display(), "admin uds listener bound");
        let b = Arc::clone(&broadcaster);
        let handle = tokio::spawn(async move {
            if let Err(e) = inferd_daemon::admin::serve_admin_uds(listener, b, shutdown_rx).await {
                error!(error = ?e, "admin uds listener error");
            }
        });
        Ok((shutdown_tx, handle))
    }
    #[cfg(windows)]
    {
        let path_str = addr
            .to_str()
            .ok_or_else(|| {
                anyhow::anyhow!("admin pipe path is not valid utf-8: {}", addr.display())
            })?
            .to_string();
        let first = inferd_daemon::endpoint::bind_admin_pipe(&path_str, true)
            .map_err(|e| anyhow::anyhow!("bind admin pipe {}: {e}", path_str))?;
        info!(path = %path_str, "admin pipe listener bound");
        let b = Arc::clone(&broadcaster);
        let handle = tokio::spawn(async move {
            if let Err(e) =
                inferd_daemon::admin::serve_admin_pipe(&path_str, first, b, shutdown_rx).await
            {
                error!(error = ?e, "admin pipe listener error");
            }
        });
        Ok((shutdown_tx, handle))
    }
}

/// Try to load the operator config file.
///
/// Behaviour:
/// - File present → load + validate; log on parse error and return None.
/// - File absent + `cli_path` was set explicitly → log and return None
///   (operator pointed at a path that doesn't exist; honour their
///   intent, don't surprise them by writing a default elsewhere).
/// - File absent + default path → write the shipped first-boot
///   default (gemma-4 generate + embeddinggemma-300m embed, both
///   `auto_pull: true`) and return the loaded result. This is what
///   makes the install-equals-work contract hold: a fresh user runs
///   the platform installer and the daemon fetches both blobs and
///   binds the inference + embed sockets without any hand-editing.
fn load_config_file(cli_path: Option<&std::path::Path>) -> Option<ConfigFile> {
    let path = cli_path
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(inferd_daemon::config_file::default_config_path);
    let was_default_path = cli_path.is_none();

    match ConfigFile::load(&path) {
        Ok(cfg) => {
            info!(path = %path.display(), "loaded config file");
            Some(cfg)
        }
        Err(inferd_daemon::config_file::ConfigError::NotFound(_)) if was_default_path => {
            match inferd_daemon::config_file::write_default_if_missing(&path) {
                Ok(true) => {
                    info!(
                        path = %path.display(),
                        "no config file at default path; wrote first-boot default \
                         (gemma-4-e4b + embeddinggemma-300m, auto_pull=true)"
                    );
                    match ConfigFile::load(&path) {
                        Ok(cfg) => Some(cfg),
                        Err(e) => {
                            error!(error = %e, "wrote default config but failed to reload");
                            None
                        }
                    }
                }
                Ok(false) => {
                    // Race: another process wrote the file between
                    // our load attempt and the write. Reload.
                    ConfigFile::load(&path).ok()
                }
                Err(e) => {
                    error!(
                        error = %e,
                        path = %path.display(),
                        "could not write default config; falling back to dev mode"
                    );
                    None
                }
            }
        }
        Err(inferd_daemon::config_file::ConfigError::NotFound(_)) => {
            info!(path = %path.display(), "no config file at explicit path; using CLI flags only");
            None
        }
        Err(e) => {
            error!(error = %e, "config file load failed");
            None
        }
    }
}

/// Apply ADR 0023 boot-time model auto-selection to the loaded config.
///
/// When `model_autoselect: "auto"` and no generation backend is pinned,
/// picks the Gemma 4 variant from the chosen accelerator's *total*
/// memory (`>= model_autoselect_min_vram_gib` GiB → 12B, else E4B) and
/// decides embed placement (GPU, or CPU under memory pressure) from
/// *free* memory. No-op when auto-select is off or a generation backend
/// is explicitly configured.
///
/// The accelerator memory query needs the `dl-backends` runtime probe.
/// Without that feature (static single-accelerator builds) there is no
/// query, so selection falls to E4B — the safe default — via `None`
/// memory inputs.
fn apply_model_autoselect(config: Option<ConfigFile>) -> Option<ConfigFile> {
    let mut cfg = config?;

    // Query accelerator memory (dl-backends only). Probe first so the
    // ggml backends are loaded before the memory read; the probe is
    // cached and re-run harmlessly by backend construction later.
    #[cfg(feature = "dl-backends")]
    let (total, free) = {
        use inferd_engine::llamacpp::{probe_accelerator, query_device_memory_for_kind};
        let kind = probe_accelerator();
        match query_device_memory_for_kind(kind) {
            Some(mem) => (Some(mem.total), mem.free),
            None => (None, None),
        }
    };
    #[cfg(not(feature = "dl-backends"))]
    let (total, free): (Option<u64>, Option<u64>) = (None, None);

    match inferd_daemon::autoselect::apply(&mut cfg, total, free) {
        Some(outcome) => {
            info!(
                tier = outcome.tier.as_str(),
                embed_on_cpu = outcome.embed_forced_cpu,
                total_vram_bytes = ?total,
                free_vram_bytes = ?free,
                "ADR 0023 model auto-select: warming {} ({}); embed on {}",
                outcome.tier.as_str(),
                if total.is_some() { "accelerator memory probed" } else { "no accelerator memory — default tier" },
                if outcome.embed_forced_cpu { "CPU (insufficient accelerator memory for gen+embed)" } else { "accelerator" },
            );
        }
        None => {
            // Off, or an explicit generation backend is pinned — nothing
            // to do. Silent to avoid noise on the common (off) path.
        }
    }
    Some(cfg)
}

/// Resolved generation transport. CLI flags win; absent both, the
/// daemon binds the platform-default generation socket
/// (`endpoint::default_addr`) so a stock install works with no
/// transport flag (ADR 0021 — one generation socket, default-bound).
/// TCP is no longer supported per ADR 0022.
enum ResolvedTransport {
    Uds(PathBuf),
    Pipe(String),
}

/// Pick the generation transport from CLI > platform default.
/// clap already enforces that no two CLI flags are set; this layer
/// handles the platform-default socket/pipe when neither is set.
fn resolve_transport(cli: &Cli, _config: Option<&ConfigFile>) -> anyhow::Result<ResolvedTransport> {
    if let Some(path) = cli.uds.as_ref() {
        return Ok(ResolvedTransport::Uds(path.clone()));
    }
    if let Some(path) = cli.pipe.as_ref() {
        return Ok(ResolvedTransport::Pipe(path.clone()));
    }
    // Default: bind the platform's standard generation socket / pipe.
    let default = inferd_daemon::endpoint::default_addr();
    #[cfg(windows)]
    {
        let p = default.to_string_lossy().to_string();
        info!(pipe = %p, "binding default generation named pipe");
        Ok(ResolvedTransport::Pipe(p))
    }
    #[cfg(not(windows))]
    {
        info!(path = %default.display(), "binding default generation socket");
        Ok(ResolvedTransport::Uds(default))
    }
}

/// Construct backends per CLI + config. Publishes `loading_model`
/// phase events as it goes.
///
/// Dispatch:
/// - No `--backend` flag + config present → use `backends:` from the
///   config file (or legacy `model:` block, auto-promoted to a single
///   llamacpp entry). Router walks them in order per ADR 0007.
/// - No `--backend` flag + no config → fall back to the in-memory
///   `mock` backend so the daemon still boots in dev mode.
/// - Explicit `--backend mock` → mock (config ignored; useful in test
///   rigs that have an unrelated config file on disk).
/// - Explicit `--backend <kind>` (any non-mock value) → CLI-flag-only
///   path; config `backends:` are ignored so the operator gets exactly
///   what they asked for.
async fn build_backends(
    cli: &Cli,
    #[cfg_attr(
        all(not(feature = "llamacpp"), not(feature = "openai")),
        allow(unused_variables)
    )]
    config: Option<&ConfigFile>,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<(Vec<Arc<dyn Backend>>, Vec<String>)> {
    // Returns the backends AND a parallel list of unique labels — the
    // config-entry name (e.g. "gemma-4-e4b" / "embeddinggemma-300m"),
    // not `Backend::name()` which is the *kind* ("llamacpp") and
    // collides across entries. The labels key the per-backend
    // capabilities frames so doctor can report each backend distinctly.
    //
    // Default path (no --backend): defer to the config file when
    // present, otherwise fall back to mock. This is the change that
    // fixes #15 — previously the unset CLI flag silently defaulted to
    // mock and short-circuited config loading.
    if cli.backend.is_none() {
        #[cfg(any(feature = "llamacpp", feature = "openai", feature = "bedrock"))]
        if let Some(cfg) = config {
            // `mut` is only used under `feature = "llamacpp"` (the
            // `entries.first_mut()` legacy-embed override below). With only
            // openai/bedrock enabled the binding is read-only — silence the
            // unused_mut warning on that feature combo without losing -D
            // warnings elsewhere.
            #[cfg_attr(not(feature = "llamacpp"), allow(unused_mut))]
            let mut entries = cfg.resolved_backends();
            // Issue #16: when the config is the legacy single-model
            // shape (`model:` at top level, not `backends:`),
            // resolved_backends() promotes it to a one-element list
            // with embed=false hard-coded. The --llamacpp-embed CLI
            // flag opts that one entry into embed without forcing the
            // operator to migrate to the multi-backend `backends:`
            // shape. We only override on the legacy promotion path
            // (cfg.model.is_some() && cfg.backends.is_none()) so an
            // explicit multi-backend config keeps full control.
            #[cfg(feature = "llamacpp")]
            if cli.llamacpp_embed
                && cfg.model.is_some()
                && cfg.backends.is_none()
                && let Some(BackendEntry::Llamacpp(e)) = entries.first_mut()
            {
                e.embed = true;
                e.embed_pooling = cli.llamacpp_embed_pooling;
                e.embed_n_ctx = cli.llamacpp_embed_n_ctx;
            }
            if !entries.is_empty() {
                let auto_pull = cfg.auto_pull;
                let mut out: Vec<Arc<dyn Backend>> = Vec::with_capacity(entries.len());
                let mut labels: Vec<String> = Vec::with_capacity(entries.len());
                for entry in entries {
                    let label = entry.name().to_string();
                    let b = build_entry(&entry, cfg, auto_pull, Arc::clone(&broadcaster)).await?;
                    out.push(b);
                    labels.push(label);
                }
                return Ok((out, labels));
            }
        }

        // No `--backend` flag and no real backend could be built. inferd
        // will NOT silently fall back to the mock backend — serving fake
        // tokens on a real install violates install=work. On a normal
        // install the daemon writes a default config (gemma-4-e4b +
        // embeddinggemma-300m, auto_pull) on first boot and loads it;
        // reaching here means that config is missing / unreadable /
        // declares no backends, or this binary was built without any
        // inference backend feature (llamacpp / openai / bedrock).
        // Mock is reachable only via an explicit `--backend mock`.
        anyhow::bail!(
            "refusing to start: no usable inference backend. No `--backend` \
             flag was given and no real backend could be constructed from \
             the config (`~/.inferd/config.json`). On a normal install the \
             daemon writes a default config on first boot and loads it — if \
             you see this, that config is missing, unreadable, or declares \
             no backends, or this binary was built without a backend feature \
             (llamacpp / openai / bedrock). Fix the config (or rebuild with \
             a backend feature), or pass `--backend mock` explicitly to run \
             a no-engine test daemon. inferd never silently serves a mock \
             backend."
        );
    }

    match cli.backend.expect("checked is_none above") {
        BackendKind::Mock => {
            broadcaster.publish(StatusEvent::LoadingModel {
                phase: LoadPhase::CheckingLocal {
                    path: PathBuf::from("(mock)"),
                },
            });
            Ok((vec![Arc::new(Mock::new())], vec!["mock".to_string()]))
        }
        #[cfg(feature = "llamacpp")]
        BackendKind::Llamacpp => {
            let b = build_llamacpp_cli_only(cli, Arc::clone(&broadcaster)).await?;
            Ok((vec![b], vec!["llamacpp".to_string()]))
        }
        #[cfg(feature = "openai")]
        BackendKind::OpenaiCompat => {
            let b = build_openai_compat_cli_only(cli, Arc::clone(&broadcaster))?;
            Ok((vec![b], vec!["openai-compat".to_string()]))
        }
        #[cfg(feature = "bedrock")]
        BackendKind::BedrockInvoke => {
            let b = build_bedrock_invoke_cli_only(cli, Arc::clone(&broadcaster))?;
            Ok((vec![b], vec!["bedrock-invoke".to_string()]))
        }
    }
}

/// Dispatch a single config-file backend entry to the right builder.
#[cfg(any(feature = "llamacpp", feature = "openai", feature = "bedrock"))]
async fn build_entry(
    entry: &BackendEntry,
    #[cfg_attr(not(feature = "llamacpp"), allow(unused_variables))] cfg: &ConfigFile,
    #[cfg_attr(not(feature = "llamacpp"), allow(unused_variables))] auto_pull: bool,
    #[cfg_attr(
        all(
            not(feature = "llamacpp"),
            not(feature = "openai"),
            not(feature = "bedrock")
        ),
        allow(unused_variables)
    )]
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    match entry {
        #[cfg(feature = "llamacpp")]
        BackendEntry::Llamacpp(e) => build_llamacpp_entry(e, cfg, auto_pull, broadcaster).await,
        #[cfg(not(feature = "llamacpp"))]
        BackendEntry::Llamacpp(_) => {
            anyhow::bail!(
                "config declares a `kind: llamacpp` backend but this daemon \
                 binary was built without the `llamacpp` feature"
            )
        }
        #[cfg(feature = "openai")]
        BackendEntry::OpenaiCompat(e) => build_openai_compat_entry(e, broadcaster),
        #[cfg(not(feature = "openai"))]
        BackendEntry::OpenaiCompat(_) => {
            anyhow::bail!(
                "config declares a `kind: openai-compat` backend but this \
                 daemon binary was built without the `openai` feature"
            )
        }
        #[cfg(feature = "bedrock")]
        BackendEntry::BedrockInvoke(e) => build_bedrock_invoke_entry(e, broadcaster),
        #[cfg(not(feature = "bedrock"))]
        BackendEntry::BedrockInvoke(_) => {
            anyhow::bail!(
                "config declares a `kind: bedrock-invoke` backend but this \
                 daemon binary was built without the `bedrock` feature"
            )
        }
    }
}

/// Build an OpenAI-compat backend from a config-file entry.
/// API key is resolved from env-var name only; no literal in config.
#[cfg(feature = "openai")]
fn build_openai_compat_entry(
    entry: &OpenaiCompatEntry,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_engine::openai_compat::{OpenAiCompat, OpenAiCompatConfig};

    let api_key = resolve_openai_api_key(entry.api_key_env.as_deref());

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal {
            path: PathBuf::from(format!(
                "(openai-compat: {} / {})",
                entry.base_url, entry.model
            )),
        },
    });

    let backend = OpenAiCompat::new(OpenAiCompatConfig {
        base_url: entry.base_url.clone(),
        api_key,
        model: entry.model.clone(),
        timeout: Duration::from_secs(entry.timeout_secs),
    })
    .map_err(|e| anyhow::anyhow!("openai-compat init failed for {}: {e}", entry.name))?;
    Ok(Arc::new(backend))
}

/// Resolve the bearer token for an openai-compat backend.
///
/// Order: explicit `api_key_env: "<NAME>"` → `INFERD_OPENAI_API_KEY`
/// → `OPENAI_API_KEY` → empty (skips `Authorization` header). Never
/// reads a literal key from the config file (THREAT_MODEL: secrets
/// stay in env, not on disk).
#[cfg(feature = "openai")]
fn resolve_openai_api_key(api_key_env: Option<&str>) -> String {
    if let Some(name) = api_key_env
        && let Ok(v) = std::env::var(name)
    {
        return v;
    }
    if let Ok(v) = std::env::var("INFERD_OPENAI_API_KEY") {
        return v;
    }
    if let Ok(v) = std::env::var("OPENAI_API_KEY") {
        return v;
    }
    String::new()
}

/// CLI-only path for `--backend openai-compat` without a config file.
/// Mirrors the v0.1.14 surface so existing scripts keep working.
#[cfg(feature = "openai")]
fn build_openai_compat_cli_only(
    cli: &Cli,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_engine::openai_compat::{OpenAiCompat, OpenAiCompatConfig};

    let base_url = cli.openai_base_url.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "--backend openai-compat requires --openai-base-url \
             (e.g. https://api.openai.com, http://localhost:11434)"
        )
    })?;
    let model = cli.openai_model.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "--backend openai-compat requires --openai-model \
             (e.g. gpt-4o-mini, llama3.1:8b)"
        )
    })?;
    let api_key = cli
        .openai_api_key
        .clone()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .unwrap_or_default();

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal {
            path: PathBuf::from(format!("(openai-compat: {base_url} / {model})")),
        },
    });

    let backend = OpenAiCompat::new(OpenAiCompatConfig {
        base_url: base_url.to_string(),
        api_key,
        model: model.to_string(),
        timeout: Duration::from_secs(cli.openai_timeout_secs),
    })
    .map_err(|e| anyhow::anyhow!("openai-compat init failed: {e}"))?;
    Ok(Arc::new(backend))
}

/// Build a bedrock-invoke backend from a config-file entry. The auth
/// credentials are read from env at startup — the config file only
/// names the env var.
#[cfg(feature = "bedrock")]
fn build_bedrock_invoke_entry(
    entry: &BedrockInvokeEntry,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_engine::bedrock_invoke::{BedrockInvoke, BedrockInvokeConfig};

    let bearer = entry
        .bearer_token_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .filter(|v| !v.is_empty());
    let auth = resolve_bedrock_auth(bearer.as_deref()).ok_or_else(|| {
        anyhow::anyhow!(
            "bedrock-invoke {:?}: no auth credentials. Set the env var named in \
             `bearer_token_env` (Bearer auth) or AWS_ACCESS_KEY_ID / \
             AWS_SECRET_ACCESS_KEY (SigV4)",
            entry.name
        )
    })?;

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal {
            path: PathBuf::from(format!(
                "(bedrock-invoke: {} / {})",
                entry.region, entry.model_id
            )),
        },
    });

    let backend = BedrockInvoke::new(BedrockInvokeConfig {
        region: entry.region.clone(),
        model_id: entry.model_id.clone(),
        auth,
        timeout: Duration::from_secs(entry.timeout_secs),
        endpoint_override: entry.endpoint.clone().filter(|s| !s.is_empty()),
    })
    .map_err(|e| anyhow::anyhow!("bedrock-invoke init failed for {}: {e}", entry.name))?;
    Ok(Arc::new(backend))
}

/// CLI-only path for `--backend bedrock-invoke` without a config file.
/// Mirrors the openai-compat CLI shape.
#[cfg(feature = "bedrock")]
fn build_bedrock_invoke_cli_only(
    cli: &Cli,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_engine::bedrock_invoke::{BedrockInvoke, BedrockInvokeConfig};

    let region = cli.bedrock_region.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "--backend bedrock-invoke requires --bedrock-region \
             (e.g. us-east-1, eu-central-1)"
        )
    })?;
    let model_id = cli.bedrock_model_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "--backend bedrock-invoke requires --bedrock-model-id \
             (e.g. anthropic.claude-3-5-sonnet-20241022-v2:0)"
        )
    })?;
    let bearer = cli
        .bedrock_bearer_token
        .as_deref()
        .filter(|v| !v.is_empty());
    let auth = resolve_bedrock_auth(bearer).ok_or_else(|| {
        anyhow::anyhow!(
            "bedrock-invoke: no auth credentials. Set --bedrock-bearer-token / \
             AWS_BEARER_TOKEN_BEDROCK (Bearer auth) or AWS_ACCESS_KEY_ID / \
             AWS_SECRET_ACCESS_KEY (SigV4)"
        )
    })?;

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal {
            path: PathBuf::from(format!("(bedrock-invoke: {region} / {model_id})")),
        },
    });

    let backend = BedrockInvoke::new(BedrockInvokeConfig {
        region: region.to_string(),
        model_id: model_id.to_string(),
        auth,
        timeout: Duration::from_secs(cli.bedrock_timeout_secs),
        endpoint_override: cli.bedrock_endpoint.clone().filter(|s| !s.is_empty()),
    })
    .map_err(|e| anyhow::anyhow!("bedrock-invoke init failed: {e}"))?;
    Ok(Arc::new(backend))
}

/// Resolve Bedrock auth from a (possibly-empty) bearer token + the
/// standard AWS env var chain. Returns `None` when neither shape is
/// satisfied.
#[cfg(feature = "bedrock")]
fn resolve_bedrock_auth(
    bearer: Option<&str>,
) -> Option<inferd_engine::bedrock_invoke::BedrockAuth> {
    use inferd_engine::bedrock_invoke::BedrockAuth;
    if let Some(token) = bearer
        && !token.is_empty()
    {
        return Some(BedrockAuth::BearerToken(token.to_string()));
    }
    let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
    let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
    if access_key_id.is_empty() || secret_access_key.is_empty() {
        return None;
    }
    let session_token = std::env::var("AWS_SESSION_TOKEN")
        .ok()
        .filter(|v| !v.is_empty());
    Some(BedrockAuth::Sigv4 {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

/// Build a llamacpp backend from a config-file entry. Resolves the
/// model into the shared CAS store (ADR 0011) and returns the verified
/// blob's path so the engine can mmap it. Publishes phase events
/// through the broadcaster throughout.
#[cfg(feature = "llamacpp")]
async fn build_llamacpp_entry(
    entry: &LlamacppEntry,
    cfg: &ConfigFile,
    auto_pull: bool,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_daemon::fetch::{ModelSpec, fetch_model};
    use inferd_daemon::store::{ModelStore, default_models_home};
    use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};

    let spec: ModelSpec = (&entry.model).into();
    let n_ctx = entry.n_ctx;
    let n_gpu_layers = entry.n_gpu_layers;
    let model_sha256_bytes = parse_sha256_hex(&entry.model.sha256)?;
    let store = match cfg.models_home.as_ref() {
        Some(p) => ModelStore::open(p),
        None => ModelStore::open(default_models_home()),
    };

    let model_path = if auto_pull {
        let spec_clone = spec.clone();
        let store_clone = store.clone();
        let bcast = Arc::clone(&broadcaster);
        tokio::task::spawn_blocking(move || fetch_model(&spec_clone, &store_clone, &bcast))
            .await
            .map_err(|e| anyhow::anyhow!("fetch task join: {e}"))?
            .map_err(|e| anyhow::anyhow!("fetch failed for {}: {e}", entry.name))?
    } else {
        let blob_path = store.blob_path(&spec.sha256_hex);
        if !blob_path.exists() {
            anyhow::bail!(
                "model {} not present in store at {} and auto_pull is disabled. \
                 Run `inferdctl pull` or set auto_pull: true in config.",
                entry.name,
                blob_path.display()
            );
        }
        broadcaster.publish(StatusEvent::LoadingModel {
            phase: LoadPhase::CheckingLocal {
                path: blob_path.clone(),
            },
        });
        blob_path
    };

    // Optional multimodal projector (issue #30). When the entry has an
    // `mmproj` block, fetch it as an additional CAS blob via the same
    // pinned-URL + constant-time-SHA path as the base model, then hand
    // its path + expected SHA to the engine so libmtmd loads it and
    // `capabilities().vision` flips true. None → text-only backend.
    let (mmproj_path, mmproj_sha256_bytes) = if let Some(mm) = entry.mmproj.as_ref() {
        let mm_spec: ModelSpec = mm.into();
        let mm_sha = parse_sha256_hex(&mm.sha256)?;
        let path = if auto_pull {
            let spec_clone = mm_spec.clone();
            let store_clone = store.clone();
            let bcast = Arc::clone(&broadcaster);
            tokio::task::spawn_blocking(move || fetch_model(&spec_clone, &store_clone, &bcast))
                .await
                .map_err(|e| anyhow::anyhow!("mmproj fetch task join: {e}"))?
                .map_err(|e| anyhow::anyhow!("mmproj fetch failed for {}: {e}", entry.name))?
        } else {
            let blob_path = store.blob_path(&mm_spec.sha256_hex);
            if !blob_path.exists() {
                anyhow::bail!(
                    "mmproj for {} not present in store at {} and auto_pull is disabled. \
                     Run `inferdctl pull` or set auto_pull: true in config.",
                    entry.name,
                    blob_path.display()
                );
            }
            blob_path
        };
        (Some(path), Some(mm_sha))
    } else {
        (None, None)
    };

    // ADR 0023 pre-load fit check: if this entry offloads to a GPU,
    // compare a conservative VRAM estimate against the accelerator's
    // *free* memory now and fail with a CLEAR, actionable message rather
    // than letting libllama surface a cryptic GPU-OOM (it reports
    // out-of-VRAM as `invalid vector subscript` /
    // `llama_model_load_from_file returned null`). CPU entries
    // (`n_gpu_layers == 0`) and static builds are exempt.
    #[cfg(feature = "dl-backends")]
    if n_gpu_layers != 0 {
        use inferd_daemon::autoselect::{Tier, estimate_embed_vram_bytes, estimate_gen_vram_bytes};
        use inferd_engine::llamacpp::{probe_accelerator, query_device_memory_for_kind};

        let kind = probe_accelerator();
        // Estimate by model size class + whether this entry is embed.
        // 12B if the pinned model is the 7 GB+ variant.
        let est = if entry.embed {
            estimate_embed_vram_bytes()
        } else {
            let big = entry
                .model
                .size_bytes
                .map(|b| b >= 7_000_000_000)
                .unwrap_or(false);
            let tier = if big { Tier::B12 } else { Tier::E4b };
            estimate_gen_vram_bytes(tier, n_ctx, mmproj_path.is_some())
        };
        const FIT_HEADROOM: u64 = 512 * 1024 * 1024; // 0.5 GiB
        if let Some(mem) = query_device_memory_for_kind(kind)
            && let Some(free) = mem.free
            && free < est.saturating_add(FIT_HEADROOM)
        {
            let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
            let suggested_ctx = if n_ctx > 8192 { 8192 } else { 4096 };
            anyhow::bail!(
                "insufficient accelerator memory for backend '{}': needs ~{:.1} GiB \
                 on {} but only {:.1} GiB is free right now. Remedies: reduce \
                 `n_ctx` (e.g. {} → {}), use the smaller Gemma 4 E4B model, disable \
                 embed on this backend, close other GPU apps, or run \
                 `INFERD_FORCE_BACKEND=cpu` (slower). \
                 (inferd checks this before load so you don't get a cryptic \
                 llama.cpp out-of-memory error.)",
                entry.name,
                gib(est),
                kind.as_str(),
                gib(free),
                n_ctx,
                suggested_ctx,
            );
        }
    }

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Mmap {
            path: model_path.clone(),
        },
    });
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::KvCache { n_ctx },
    });

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path,
        model_sha256: Some(model_sha256_bytes),
        mmproj_path,
        mmproj_sha256: mmproj_sha256_bytes,
        mmproj_image_max_tokens: entry.mmproj_image_max_tokens,
        n_ctx,
        n_gpu_layers,
        embed: entry.embed,
        embed_pooling: entry.embed_pooling,
        embed_n_ctx: entry.embed_n_ctx,
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("llamacpp init failed for {}: {e}", entry.name))?;
    Ok(Arc::new(backend))
}

/// CLI-only path for `--backend llamacpp` without a config file. The
/// operator points us at a specific file via `--model-path` and the
/// daemon bypasses the CAS store / fetch entirely. Useful for dev /
/// CI.
#[cfg(feature = "llamacpp")]
async fn build_llamacpp_cli_only(
    cli: &Cli,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};

    let path = cli.model_path.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "--backend llamacpp needs either ~/.inferd/config.json or \
             --model-path/--model-sha256 CLI flags"
        )
    })?;
    let sha_str = cli
        .model_sha256
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--model-sha256 is required for --backend llamacpp"))?;
    let model_sha256_bytes = parse_sha256_hex(sha_str)?;

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal { path: path.clone() },
    });
    if !path.exists() {
        anyhow::bail!("model not present at {} (CLI-only mode)", path.display());
    }

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Mmap { path: path.clone() },
    });
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::KvCache { n_ctx: cli.n_ctx },
    });

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path.clone(),
        model_sha256: Some(model_sha256_bytes),
        n_ctx: cli.n_ctx,
        n_gpu_layers: cli.n_gpu_layers,
        // CLI-only path: --llamacpp-embed / --llamacpp-embed-pooling
        // / --llamacpp-embed-n-ctx flow straight into the config so
        // dev-mode rigs without a config file can still serve embed.
        // Issue #16 fix.
        embed: cli.llamacpp_embed,
        embed_pooling: cli.llamacpp_embed_pooling,
        embed_n_ctx: cli.llamacpp_embed_n_ctx,
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("llamacpp init failed: {e}"))?;
    Ok(Arc::new(backend))
}

#[cfg(feature = "llamacpp")]
fn parse_sha256_hex(s: &str) -> anyhow::Result<[u8; 32]> {
    if s.len() != 64 {
        anyhow::bail!("model.sha256 must be 64 hex chars (got {})", s.len());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_digit(s.as_bytes()[i * 2])?;
        let lo = hex_digit(s.as_bytes()[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Ok(out)
}

#[cfg(feature = "llamacpp")]
fn hex_digit(b: u8) -> anyhow::Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => anyhow::bail!("invalid hex digit: {:?}", b as char),
    }
}

/// Initialise tracing with two layers:
/// - stderr `fmt` layer for operators tailing the daemon (compact, plain).
/// - `LogxLayer` writing NDJSON to the activity log under
///   `default_log_dir()` (or `INFERD_LOG_DIR` when set), with the secret
///   redactor applied per-record.
///
/// `INFERD_LOG` controls verbosity; default is `info`. Set to `debug`
/// for verbose request/response capture; set to `0`/`off` to silence
/// everything.
fn install_tracing() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("INFERD_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .compact();

    let log_dir = default_log_dir();
    let writer = Arc::new(LogxWriter::open(&log_dir, "inferd", DEFAULT_ROTATE_BYTES)?);
    let logx_layer = LogxLayer::new(writer);

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(logx_layer)
        .init();
    Ok(())
}

/// Free a console window the daemon owns, so a Startup-shortcut launch
/// doesn't leave a tracing window on the desktop (issue #28).
///
/// `GetConsoleProcessList` reports how many processes share the attached
/// console. A console allocated *for us* by `CreateProcess` (the shortcut /
/// double-click case) lists exactly one PID — ours. A console inherited
/// from an interactive shell lists at least two (the shell + us), and we
/// leave that one alone so `inferd-daemon` run by hand still prints to the
/// terminal. `FreeConsole` only detaches this process from the console; the
/// file + admin-pipe log sinks installed by `install_tracing` are
/// unaffected. If no console is attached, `GetConsoleProcessList` returns 0
/// and we no-op.
#[cfg(windows)]
fn detach_own_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    // SAFETY: GetConsoleProcessList writes up to `len` PIDs into the buffer
    // and returns the count actually attached (0 if no console). We pass a
    // small fixed buffer; we only care whether the count is exactly 1.
    let mut pids = [0u32; 4];
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    if count == 1 {
        // SAFETY: detaches this process from its console. No further console
        // I/O is expected; tracing writes to the activity log + admin pipe.
        unsafe {
            FreeConsole();
        }
    }
}

/// Wire Ctrl-C (SIGINT on Unix) to N oneshot channels so multiple
/// accept loops exit cleanly on the same signal. On Unix we
/// additionally listen for SIGTERM. Returns one receiver per
/// requested fan-out — e.g. 2 when both v1 and v2 listeners are
/// running.
fn install_shutdown_signal(
    fanout: usize,
) -> anyhow::Result<Vec<tokio::sync::oneshot::Receiver<()>>> {
    let (txs, rxs): (Vec<_>, Vec<_>) = (0..fanout).map(|_| tokio::sync::oneshot::channel()).unzip();

    tokio::spawn(async move {
        #[cfg(unix)]
        let result: Result<(), std::io::Error> = async {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = signal(SignalKind::terminate())?;
            let mut sigint = signal(SignalKind::interrupt())?;
            tokio::select! {
                _ = sigterm.recv() => {},
                _ = sigint.recv() => {},
            }
            Ok(())
        }
        .await;
        #[cfg(not(unix))]
        let result: Result<(), std::io::Error> = tokio::signal::ctrl_c().await;

        if let Err(e) = result {
            error!(error = ?e, "signal handler failed; shutdown channels will not fire");
            return;
        }
        for tx in txs {
            let _ = tx.send(());
        }
    });

    Ok(rxs)
}
