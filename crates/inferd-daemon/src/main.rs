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
use inferd_daemon::config_file::ConfigFile;
#[cfg(unix)]
use inferd_daemon::endpoint::bind_admin_uds;
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
use inferd_daemon::endpoint::{bind_tcp, default_admin_addr};
use inferd_daemon::lifecycle::{AcceptContext, serve_tcp, wait_for_ready};
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

    let cli = Cli::parse();
    cli.require_one_transport()
        .map_err(|m| anyhow::anyhow!("{m}"))?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "inferd-daemon starting"
    );

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

    // Resolve model + construct backend. Publishes loading_model
    // phase events through the broadcaster.
    let backend: Arc<dyn Backend> =
        match build_backend(&cli, config.as_ref(), Arc::clone(&broadcaster)).await {
            Ok(b) => b,
            Err(e) => {
                let _ = admin_shutdown_tx.send(());
                let _ = tokio::time::timeout(Duration::from_secs(1), admin_handle).await;
                return Err(e);
            }
        };
    info!(name = backend.name(), "backend constructed");

    // Build router (no-op v0.1: one backend).
    let router = Arc::new(Router::new(vec![Arc::clone(&backend)]));

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

    // Inference shutdown channel.
    let inference_shutdown_tx = install_shutdown_signal()?;

    let admission = inferd_daemon::queue::Admission::new(cli.active_permits, cli.queue_depth);
    info!(
        active_permits = cli.active_permits,
        queue_depth = cli.queue_depth,
        capacity = admission.capacity(),
        "admission gate configured"
    );

    let accept_ctx = AcceptContext {
        expected_api_key: cli.api_key.clone(),
        admission: Some(admission),
    };
    if cli.tcp.is_some() && accept_ctx.expected_api_key.is_some() {
        info!("tcp api-key auth enabled (F-8)");
    } else if cli.tcp.is_some() {
        warn!(
            "tcp listener has no --api-key configured; any local process \
             can connect (THREAT_MODEL F-8)"
        );
    }

    let serve_result = if let Some(addr) = cli.tcp.as_deref() {
        let listener = bind_tcp(addr).await?;
        info!(addr = %listener.local_addr()?, "tcp listener bound");
        serve_tcp(listener, router, accept_ctx, inference_shutdown_tx).await
    } else if let Some(path) = cli.uds.as_ref() {
        #[cfg(unix)]
        {
            let listener = bind_uds(path, cli.group.as_deref()).await?;
            info!(path = %path.display(), "uds listener bound");
            inferd_daemon::lifecycle::serve_uds(listener, router, accept_ctx, inference_shutdown_tx)
                .await
        }
        #[cfg(not(unix))]
        {
            drop((path, router, accept_ctx, inference_shutdown_tx));
            anyhow::bail!(
                "Unix domain sockets are not supported on this platform; use --pipe or --tcp"
            );
        }
    } else if let Some(path) = cli.pipe.as_ref() {
        #[cfg(windows)]
        {
            let first = inferd_daemon::endpoint::bind_named_pipe(path, true)?;
            info!(path = %path, "named pipe listener bound");
            inferd_daemon::lifecycle::serve_named_pipe(
                path,
                first,
                router,
                accept_ctx,
                inference_shutdown_tx,
            )
            .await
        }
        #[cfg(not(windows))]
        {
            drop((path, router, accept_ctx, inference_shutdown_tx));
            anyhow::bail!(
                "Windows named pipes are not supported on this platform; use --uds or --tcp"
            );
        }
    } else {
        unreachable!("require_one_transport already verified");
    };

    // Drain: tell admin subscribers we're going away, then close.
    broadcaster.publish(StatusEvent::Draining);
    let _ = admin_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), admin_handle).await;

    serve_result?;
    info!("shutdown complete");
    Ok(())
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

/// Try to load the operator config file. Returns `None` (and logs at
/// info level) if the file doesn't exist — that's dev mode against
/// CLI flags. Returns `Err` only on I/O / parse / validation failure
/// of an existing file.
fn load_config_file(cli_path: Option<&std::path::Path>) -> Option<ConfigFile> {
    let path = cli_path
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(inferd_daemon::config_file::default_config_path);
    match ConfigFile::load(&path) {
        Ok(cfg) => {
            info!(path = %path.display(), "loaded config file");
            Some(cfg)
        }
        Err(inferd_daemon::config_file::ConfigError::NotFound(_)) => {
            info!(path = %path.display(), "no config file; using CLI flags only");
            None
        }
        Err(e) => {
            error!(error = %e, "config file load failed");
            None
        }
    }
}

/// Construct the backend per CLI + config. Publishes `loading_model`
/// phase events as it goes.
async fn build_backend(
    cli: &Cli,
    #[cfg_attr(not(feature = "llamacpp"), allow(unused_variables))] config: Option<&ConfigFile>,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    match cli.backend {
        BackendKind::Mock => {
            // Mock: no model on disk, no fetch, ready immediately.
            // Still publish the lifecycle phases so admin subscribers
            // see the same shape as the production path.
            broadcaster.publish(StatusEvent::LoadingModel {
                phase: LoadPhase::CheckingLocal {
                    path: PathBuf::from("(mock)"),
                },
            });
            Ok(Arc::new(Mock::new()))
        }
        #[cfg(feature = "llamacpp")]
        BackendKind::Llamacpp => build_llamacpp(cli, config, broadcaster).await,
    }
}

/// Resolve the model into the shared CAS store (ADR 0011). On
/// success returns the verified blob's path so the engine can mmap
/// it. Publishes phase events through the broadcaster throughout.
#[cfg(feature = "llamacpp")]
async fn build_llamacpp(
    cli: &Cli,
    config: Option<&ConfigFile>,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Arc<dyn Backend>> {
    use inferd_daemon::fetch::{ModelSpec, fetch_model};
    use inferd_daemon::store::{ModelStore, default_models_home};
    use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};

    // Resolve the spec + store: prefer config-file, fall back to CLI flags.
    let (spec, store, n_ctx, n_gpu_layers, model_sha256_bytes, cli_only_path) = match config {
        Some(cfg) => {
            let spec: ModelSpec = (&cfg.model).into();
            let n_ctx = if cli.n_ctx != 8192 {
                cli.n_ctx
            } else {
                cfg.n_ctx
            };
            let n_gpu_layers = if cli.n_gpu_layers != 0 {
                cli.n_gpu_layers
            } else {
                cfg.n_gpu_layers
            };
            let sha = parse_sha256_hex(&cfg.model.sha256)?;
            let store = match cfg.models_home.as_ref() {
                Some(p) => ModelStore::open(p),
                None => ModelStore::open(default_models_home()),
            };
            (spec, store, n_ctx, n_gpu_layers, sha, None)
        }
        None => {
            let path = cli.model_path.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--backend llamacpp needs either ~/.inferd/config.json or \
                     --model-path/--model-sha256 CLI flags"
                )
            })?;
            let sha_str = cli.model_sha256.as_ref().ok_or_else(|| {
                anyhow::anyhow!("--model-sha256 is required for --backend llamacpp")
            })?;
            let sha = parse_sha256_hex(sha_str)?;
            // CLI-only mode: bypass the CAS store; the operator
            // pointed us at a specific file. Useful for dev / CI.
            let spec = ModelSpec {
                name: "cli".into(),
                source_url: String::new(),
                sha256_hex: sha_str.clone(),
                size_bytes: None,
                license: None,
                source: None,
            };
            // Store is constructed but unused in this branch.
            (
                spec,
                ModelStore::open(default_models_home()),
                cli.n_ctx,
                cli.n_gpu_layers,
                sha,
                Some(path.clone()),
            )
        }
    };

    // Resolve / fetch.
    let auto_pull = config.map(|c| c.auto_pull).unwrap_or(false);
    let model_path = if let Some(direct) = cli_only_path {
        // CLI-only mode: file must be at --model-path. No CAS lookup,
        // no fetch.
        broadcaster.publish(StatusEvent::LoadingModel {
            phase: LoadPhase::CheckingLocal {
                path: direct.clone(),
            },
        });
        if !direct.exists() {
            anyhow::bail!("model not present at {} (CLI-only mode)", direct.display());
        }
        direct
    } else if auto_pull {
        // Run the (synchronous) fetch on a blocking thread so we
        // don't block the tokio runtime.
        let spec_clone = spec.clone();
        let store_clone = store.clone();
        let bcast = Arc::clone(&broadcaster);
        tokio::task::spawn_blocking(move || fetch_model(&spec_clone, &store_clone, &bcast))
            .await
            .map_err(|e| anyhow::anyhow!("fetch task join: {e}"))?
            .map_err(|e| anyhow::anyhow!("fetch failed: {e}"))?
    } else {
        // auto_pull == false: blob must already exist in the CAS at
        // its SHA path. fetch_model() returns immediately when the
        // manifest + blob agree, so we still call it — it does no
        // network when source_url is unset OR the cached blob
        // matches.
        let blob_path = store.blob_path(&spec.sha256_hex);
        if !blob_path.exists() {
            anyhow::bail!(
                "model not present in store at {} and auto_pull is disabled. \
                 Run `inferdctl pull` or set auto_pull: true in config.",
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

    // Mmap phase.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Mmap {
            path: model_path.clone(),
        },
    });
    // KV cache phase.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::KvCache { n_ctx },
    });

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path,
        model_sha256: Some(model_sha256_bytes),
        n_ctx,
        n_gpu_layers,
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

/// Wire Ctrl-C (SIGINT on Unix) to a oneshot channel so the accept loop
/// exits cleanly. On Unix we additionally listen for SIGTERM.
fn install_shutdown_signal() -> anyhow::Result<tokio::sync::oneshot::Receiver<()>> {
    let (tx, rx) = tokio::sync::oneshot::channel();

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
            error!(error = ?e, "signal handler failed; shutdown channel will not fire");
            return;
        }
        let _ = tx.send(());
    });

    Ok(rx)
}
