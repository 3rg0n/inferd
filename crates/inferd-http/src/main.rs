//! `inferd-http` — OpenAI-compatible HTTP bridge for inferd.
//!
//! A user-launched, standalone process (ADR 0020 Surface A). It exposes
//! `/v1/chat/completions` and `/v1/embeddings` over localhost and
//! translates them to the daemon's native v2/embed IPC wire via
//! `inferd-client`. The daemon never serves HTTP (ADR 0006/0022); this
//! bridge is a consumer, not a privileged surface (ADR 0014).
//!
//! Concurrency: the bridge dials a **fresh** `ClientV2` / `EmbedClient`
//! per HTTP request (a cheap UDS/pipe connect) and lets the daemon's
//! admission queue multiplex — `ClientV2` is not `Clone` and serialises
//! generations through one connection, so a shared client would
//! bottleneck. State holds only addresses + config.

#![deny(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod config;
mod error;
mod handlers;
mod translate;

use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// Shared handler state: daemon endpoint addresses + advertised model.
/// No live clients — each request dials its own (see module docs).
#[derive(Clone)]
pub(crate) struct AppState {
    inner: Arc<AppStateInner>,
}

pub(crate) struct AppStateInner {
    pub gen_addr: config::Endpoint,
    pub embed_addr: config::Endpoint,
    pub model_name: String,
    pub startup_timeout: std::time::Duration,
}

impl AppState {
    fn new(cfg: &Config) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                gen_addr: cfg.gen_addr.clone(),
                embed_addr: cfg.embed_addr.clone(),
                model_name: cfg.model_name.clone(),
                startup_timeout: cfg.startup_timeout,
            }),
        }
    }
    pub(crate) fn gen_addr(&self) -> &config::Endpoint {
        &self.inner.gen_addr
    }
    pub(crate) fn embed_addr(&self) -> &config::Endpoint {
        &self.inner.embed_addr
    }
    pub(crate) fn model_name(&self) -> &str {
        &self.inner.model_name
    }
    pub(crate) fn startup_timeout(&self) -> std::time::Duration {
        self.inner.startup_timeout
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("INFERD_HTTP_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!(
        listen = %cfg.listen,
        gen = %cfg.gen_addr,
        embed = %cfg.embed_addr,
        model = %cfg.model_name,
        auth = cfg.token.is_some(),
        "inferd-http starting"
    );

    // Safety rail: refuse to bind a non-loopback address without a token.
    // Localhost is the no-auth default; exposing the bridge on the network
    // requires an explicit bearer token (ADR 0020 — auth terminates at the
    // bridge, and we don't silently expose an unauthenticated surface).
    if !cfg.listen.ip().is_loopback() && cfg.token.is_none() {
        anyhow::bail!(
            "refusing to bind non-loopback address {} without --token: an \
             unauthenticated network-exposed bridge is a footgun. Pass \
             --token <secret> to serve non-loopback, or bind 127.0.0.1.",
            cfg.listen
        );
    }

    let state = AppState::new(&cfg);
    let app = handlers::router(state, cfg.token.clone());

    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("bind {}", cfg.listen))?;
    tracing::info!(addr = %cfg.listen, "listening");

    axum::serve(listener, app)
        .await
        .context("http server error")?;
    Ok(())
}
