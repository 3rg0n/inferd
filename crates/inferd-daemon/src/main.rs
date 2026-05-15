//! `inferd-daemon` binary entrypoint.

use clap::Parser;
use inferd_daemon::config::{BackendKind, Cli};
use inferd_daemon::endpoint::bind_tcp;
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
use inferd_daemon::lifecycle::{serve_tcp, wait_for_ready};
use inferd_daemon::lock::Lock;
use inferd_daemon::router::Router;
use inferd_engine::{mock::Mock, Backend};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("INFERD_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    let cli = Cli::parse();
    cli.require_one_transport()
        .map_err(|m| anyhow::anyhow!("{m}"))?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "inferd-daemon starting"
    );

    // 1. Acquire single-instance lock (THREAT_MODEL F-2).
    let _lock =
        Lock::acquire(&cli.lock).map_err(|e| anyhow::anyhow!("lock acquire failed: {e}"))?;
    info!(path = %cli.lock.display(), "single-instance lock held");

    // 2. Initialise backend.
    let backend: Arc<dyn Backend> = match cli.backend {
        BackendKind::Mock => Arc::new(Mock::new()),
    };
    info!(name = backend.name(), "backend constructed");

    // 3. Build router (no-op v0.1: one backend).
    let router = Arc::new(Router::new(vec![Arc::clone(&backend)]));

    // 4. Wait for ready (THREAT_MODEL F-13).
    let waited = wait_for_ready(&router, Duration::from_secs(cli.ready_timeout_secs))
        .await
        .map_err(|e| anyhow::anyhow!("backend ready: {e}"))?;
    info!(?waited, "backend ready");

    // 5. Bind listener AFTER ready.
    let shutdown_tx = install_shutdown_signal()?;

    if let Some(addr) = cli.tcp.as_deref() {
        let listener = bind_tcp(addr).await?;
        info!(addr = %listener.local_addr()?, "tcp listener bound");
        serve_tcp(listener, router, shutdown_tx).await?;
    } else if let Some(path) = cli.uds.as_ref() {
        #[cfg(unix)]
        {
            let listener = bind_uds(path, cli.group.as_deref()).await?;
            info!(path = %path.display(), "uds listener bound");
            inferd_daemon::lifecycle::serve_uds(listener, router, shutdown_tx).await?;
        }
        #[cfg(not(unix))]
        {
            // bind_uds returns Unsupported on Windows; surface that with the
            // right exit shape rather than silently flowing past.
            drop((path, router, shutdown_tx));
            anyhow::bail!(
                "Unix domain sockets are not supported on this platform; use --tcp instead"
            );
        }
    }

    info!("shutdown complete");
    Ok(())
}

/// Wire Ctrl-C (SIGINT on Unix) to a oneshot channel so the accept loop
/// exits cleanly. On Unix we additionally listen for SIGTERM.
fn install_shutdown_signal() -> anyhow::Result<tokio::sync::oneshot::Receiver<()>> {
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        #[cfg(unix)]
        let result: Result<(), std::io::Error> = async {
            use tokio::signal::unix::{signal, SignalKind};
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
