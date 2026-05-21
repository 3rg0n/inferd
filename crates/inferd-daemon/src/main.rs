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

    // Resolve the v1 transport. CLI flags always win; when none is
    // set, fall back to `listen.tcp` from the config file. Phase
    // 6B-4: TCP is opt-in by default, declared via config not CLI.
    let resolved_v1 = resolve_v1_transport(&cli, config.as_ref())?;
    let resolved_v2_tcp: Option<String> = cli.v2_tcp.clone().or_else(|| {
        config
            .as_ref()
            .and_then(|c| c.listen.as_ref())
            .and_then(|l| l.tcp_v2.clone())
    });
    // API key resolution for TCP. CLI flag → config.listen.api_key_env
    // → INFERD_API_KEY (already wired through clap's env=). The CLI
    // value (which is the result of the clap layer) is used as the
    // default; only fall back to config when the CLI didn't set it.
    let effective_api_key: Option<String> = cli.api_key.clone().or_else(|| {
        config
            .as_ref()
            .and_then(|c| c.listen.as_ref())
            .and_then(|l| l.api_key_env.as_deref())
            .and_then(|name| std::env::var(name).ok())
            .filter(|v| !v.is_empty())
    });

    // Resolve models + construct backends. Publishes loading_model
    // phase events through the broadcaster. Returns the canonical
    // ordered list (multi-backend per ADR 0007).
    let backends: Vec<Arc<dyn Backend>> =
        match build_backends(&cli, config.as_ref(), Arc::clone(&broadcaster)).await {
            Ok(b) => b,
            Err(e) => {
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
    for b in &backends {
        let caps = b.capabilities();
        broadcaster.publish(StatusEvent::Capabilities {
            backend: b.name().to_string(),
            v2: caps.v2,
            vision: caps.vision,
            audio: caps.audio,
            tools: caps.tools,
            thinking: caps.thinking,
            accelerator: caps.accelerator.kind.as_str().to_string(),
            gpu_layers: caps.accelerator.gpu_layers,
        });
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

    // Inference shutdown channels — one per listener (v1 always, v2
    // when enabled).
    let fanout = if cli.v2 { 2 } else { 1 };
    let mut shutdown_rxs = install_shutdown_signal(fanout)?;
    let inference_shutdown_tx = shutdown_rxs.remove(0);
    let v2_shutdown_tx = if cli.v2 {
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

    let accept_ctx = AcceptContext {
        expected_api_key: effective_api_key.clone(),
        admission: Some(admission),
    };
    let any_tcp = matches!(resolved_v1, ResolvedTransport::Tcp(_)) || resolved_v2_tcp.is_some();
    if any_tcp && accept_ctx.expected_api_key.is_some() {
        info!("tcp api-key auth enabled (F-8)");
    } else if any_tcp {
        warn!(
            "tcp listener has no api-key configured (CLI --api-key or \
             config listen.api_key_env unset); any local process can \
             connect (THREAT_MODEL F-8)"
        );
    }

    // Spawn the v2 listener if enabled. It runs in parallel with the
    // v1 main accept loop and shuts down on the same signal. v1 and
    // v2 share the same Router instance — a single warm model serves
    // both wire versions.
    let v2_handle = if let Some(rx) = v2_shutdown_tx {
        Some(
            spawn_v2_listener(
                &cli,
                resolved_v2_tcp.as_deref(),
                Arc::clone(&router),
                accept_ctx.clone(),
                rx,
            )
            .await?,
        )
    } else {
        None
    };

    let serve_result = match resolved_v1 {
        ResolvedTransport::Tcp(addr) => {
            let listener = bind_tcp(&addr).await?;
            info!(addr = %listener.local_addr()?, "tcp listener bound");
            serve_tcp(listener, router, accept_ctx, inference_shutdown_tx).await
        }
        #[cfg(unix)]
        ResolvedTransport::Uds(path) => {
            let listener = bind_uds(&path, cli.group.as_deref()).await?;
            info!(path = %path.display(), "uds listener bound");
            inferd_daemon::lifecycle::serve_uds(listener, router, accept_ctx, inference_shutdown_tx)
                .await
        }
        #[cfg(not(unix))]
        ResolvedTransport::Uds(path) => {
            drop((path, router, accept_ctx, inference_shutdown_tx));
            anyhow::bail!(
                "Unix domain sockets are not supported on this platform; use --pipe or --tcp"
            );
        }
        #[cfg(windows)]
        ResolvedTransport::Pipe(path) => {
            let first = inferd_daemon::endpoint::bind_named_pipe(&path, true)?;
            info!(path = %path, "named pipe listener bound");
            inferd_daemon::lifecycle::serve_named_pipe(
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
            anyhow::bail!(
                "Windows named pipes are not supported on this platform; use --uds or --tcp"
            );
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

    serve_result?;
    info!("shutdown complete");
    Ok(())
}

/// Bind the v2 inference listener and spawn its accept loop. Returns
/// the JoinHandle so main can await graceful drain. v2 is per ADR
/// 0015: separate socket from v1; reuses the same admission gate +
/// API key + Router. Backends that don't advertise
/// `BackendCapabilities::v2 == true` see their dispatched v2
/// requests respond with `Error{Internal, "backend ... does not
/// advertise v2 capability"}`.
async fn spawn_v2_listener(
    cli: &Cli,
    resolved_v2_tcp: Option<&str>,
    router: Arc<Router>,
    accept_ctx: AcceptContext,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    use inferd_daemon::endpoint::default_v2_addr;
    use inferd_daemon::lifecycle_v2;

    if let Some(addr) = resolved_v2_tcp {
        let listener = bind_tcp(addr).await?;
        info!(addr = %listener.local_addr()?, "v2 tcp listener bound");
        Ok(tokio::spawn(async move {
            if let Err(e) =
                lifecycle_v2::serve_tcp_v2(listener, router, accept_ctx, shutdown_rx).await
            {
                error!(error = ?e, "v2 tcp listener error");
            }
        }))
    } else {
        let path = cli.v2_addr.clone().unwrap_or_else(default_v2_addr);
        #[cfg(unix)]
        {
            let listener = bind_uds(&path, cli.group.as_deref()).await?;
            info!(path = %path.display(), "v2 uds listener bound");
            Ok(tokio::spawn(async move {
                if let Err(e) =
                    lifecycle_v2::serve_uds_v2(listener, router, accept_ctx, shutdown_rx).await
                {
                    error!(error = ?e, "v2 uds listener error");
                }
            }))
        }
        #[cfg(windows)]
        {
            let path_str = path
                .to_str()
                .ok_or_else(|| {
                    anyhow::anyhow!("v2 pipe path is not valid utf-8: {}", path.display())
                })?
                .to_string();
            let first = inferd_daemon::endpoint::bind_named_pipe(&path_str, true)?;
            info!(path = %path_str, "v2 named pipe listener bound");
            Ok(tokio::spawn(async move {
                if let Err(e) = lifecycle_v2::serve_named_pipe_v2(
                    &path_str,
                    first,
                    router,
                    accept_ctx,
                    shutdown_rx,
                )
                .await
                {
                    error!(error = ?e, "v2 named pipe listener error");
                }
            }))
        }
        #[cfg(not(any(unix, windows)))]
        {
            drop((path, router, accept_ctx, shutdown_rx));
            anyhow::bail!("v2 endpoint requires unix or windows; use --v2-tcp instead")
        }
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

/// Resolved v1 inference transport. CLI flags win; config-file
/// `listen.tcp` is the fallback (Phase 6B-4 — TCP is opt-in).
enum ResolvedTransport {
    Tcp(String),
    Uds(PathBuf),
    Pipe(String),
}

/// Pick the v1 transport from CLI > config > error. clap already
/// enforces that no two CLI flags are set; this layer adds the
/// config-file fallback for `listen.tcp` and produces a clear error
/// when neither source supplies a transport.
fn resolve_v1_transport(
    cli: &Cli,
    config: Option<&ConfigFile>,
) -> anyhow::Result<ResolvedTransport> {
    if let Some(addr) = cli.tcp.as_deref() {
        return Ok(ResolvedTransport::Tcp(addr.to_string()));
    }
    if let Some(path) = cli.uds.as_ref() {
        return Ok(ResolvedTransport::Uds(path.clone()));
    }
    if let Some(path) = cli.pipe.as_ref() {
        return Ok(ResolvedTransport::Pipe(path.clone()));
    }
    if let Some(addr) = config
        .and_then(|c| c.listen.as_ref())
        .and_then(|l| l.tcp.as_deref())
    {
        info!(addr = %addr, "tcp listener from config (listen.tcp)");
        return Ok(ResolvedTransport::Tcp(addr.to_string()));
    }
    anyhow::bail!(
        "no transport configured: pass --tcp / --uds / --pipe on the CLI, \
         or set `listen.tcp` in ~/.inferd/config.json"
    )
}

/// Construct backends per CLI + config. Publishes `loading_model`
/// phase events as it goes.
///
/// Dispatch:
/// - `--backend mock` ignores the config (dev-mode echo daemon).
/// - When the config file declares `backends:` (or legacy `model:`,
///   which auto-promotes to a one-element llamacpp list), every entry
///   is built and the router walks them in order per ADR 0007.
/// - When no config is present, the CLI-flag path builds a single
///   backend matching `--backend <kind>` for v0.1.x compatibility.
async fn build_backends(
    cli: &Cli,
    #[cfg_attr(
        all(not(feature = "llamacpp"), not(feature = "openai")),
        allow(unused_variables)
    )]
    config: Option<&ConfigFile>,
    broadcaster: Arc<StatusBroadcaster>,
) -> anyhow::Result<Vec<Arc<dyn Backend>>> {
    match cli.backend {
        BackendKind::Mock => {
            broadcaster.publish(StatusEvent::LoadingModel {
                phase: LoadPhase::CheckingLocal {
                    path: PathBuf::from("(mock)"),
                },
            });
            Ok(vec![Arc::new(Mock::new())])
        }
        #[cfg(any(feature = "llamacpp", feature = "openai", feature = "bedrock"))]
        _ => {
            if let Some(cfg) = config {
                let entries = cfg.resolved_backends();
                let auto_pull = cfg.auto_pull;
                let mut out: Vec<Arc<dyn Backend>> = Vec::with_capacity(entries.len());
                for entry in entries {
                    let b = build_entry(&entry, cfg, auto_pull, Arc::clone(&broadcaster)).await?;
                    out.push(b);
                }
                return Ok(out);
            }

            // No config file: CLI-flag-only path. Single backend
            // matching `--backend <kind>`.
            match cli.backend {
                BackendKind::Mock => unreachable!("handled above"),
                #[cfg(feature = "llamacpp")]
                BackendKind::Llamacpp => {
                    let b = build_llamacpp_cli_only(cli, Arc::clone(&broadcaster)).await?;
                    Ok(vec![b])
                }
                #[cfg(feature = "openai")]
                BackendKind::OpenaiCompat => {
                    let b = build_openai_compat_cli_only(cli, Arc::clone(&broadcaster))?;
                    Ok(vec![b])
                }
                #[cfg(feature = "bedrock")]
                BackendKind::BedrockInvoke => {
                    let b = build_bedrock_invoke_cli_only(cli, Arc::clone(&broadcaster))?;
                    Ok(vec![b])
                }
            }
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
                 Run `inferd pull` or set auto_pull: true in config.",
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
        n_ctx,
        n_gpu_layers,
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
