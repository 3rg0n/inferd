//! The outbound HTTPS client both cloud adapters share (ADR 0030).
//!
//! reqwest 0.13's `rustls` feature hard-wires the aws-lc-rs provider, and
//! its `rustls-no-provider` feature panics at `Client` build time unless
//! a process-wide `CryptoProvider` was installed first. A library cannot
//! own that global, so the adapters hand reqwest a fully built rustls
//! config instead: the `ring` provider, chain verification delegated to
//! the operating system (`rustls-platform-verifier`). Building it here
//! keeps the two adapters from drifting on TLS posture.

use std::sync::Arc;
use std::time::Duration;

/// Failure to build the TLS configuration or the client around it.
#[derive(Debug, thiserror::Error)]
pub enum TlsSetupError {
    /// rustls rejected the configuration, or the platform verifier could
    /// not initialise (e.g. a Linux host with no readable root store).
    #[error("tls config: {0}")]
    Rustls(#[from] rustls::Error),
    /// reqwest rejected the client build.
    #[error("http client: {0}")]
    Client(#[from] reqwest::Error),
}

/// Build the HTTPS client an outbound adapter uses, with `timeout` as the
/// whole-request timeout.
pub(crate) fn https_client(timeout: Duration) -> Result<reqwest::Client, TlsSetupError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(Arc::clone(&provider))?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        // `dangerous()` is rustls' name for "supply your own verifier";
        // the one supplied is the OS verifier, not a permissive one.
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    // reqwest is built without its `http2` feature, so offer HTTP/1.1
    // only — the same protocol the adapters spoke on reqwest 0.12.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    Ok(reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .timeout(timeout)
        .build()?)
}
