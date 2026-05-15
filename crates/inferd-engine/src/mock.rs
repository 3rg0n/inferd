//! Deterministic mock backend used by tests and by the daemon's M1 echo
//! milestone.
//!
//! Configurable knobs cover the failure modes adapters must support:
//! - `ready` flag toggles `Backend::ready()` for testing the listener-gate
//!   invariant (`THREAT_MODEL.md` F-13).
//! - `pre_stream_error` causes `generate()` to return `GenerateError`
//!   before yielding any tokens.
//! - `mid_stream_drop_after` truncates the stream after N tokens (no
//!   `Done` event) to exercise the mid-stream failure path.

use crate::backend::{Backend, GenerateError, TokenEvent, TokenStream};
use async_trait::async_trait;
use inferd_proto::{Resolved, StopReason, Usage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

/// Configuration for `Mock` failure-mode injection.
#[derive(Debug, Clone, Default)]
pub struct MockConfig {
    /// If `Some`, `generate()` returns this error immediately. Defaults to
    /// `None` (success).
    pub pre_stream_error: Option<MockError>,
    /// If `Some(N)`, the stream yields N tokens then ends without a `Done`
    /// event, simulating a mid-stream backend failure.
    pub mid_stream_drop_after: Option<usize>,
    /// Tokens to emit (if `mid_stream_drop_after` is `None` they all stream
    /// followed by a `Done`). Default: a single canned response so callers
    /// without a config still get something useful.
    pub tokens: Vec<String>,
}

/// Variants for `MockConfig::pre_stream_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    /// Backend reports not ready.
    NotReady,
    /// Backend reports invalid request.
    InvalidRequest,
    /// Backend reports unavailable.
    Unavailable,
}

impl From<MockError> for GenerateError {
    fn from(e: MockError) -> Self {
        match e {
            MockError::NotReady => GenerateError::NotReady,
            MockError::InvalidRequest => GenerateError::InvalidRequest("mock".into()),
            MockError::Unavailable => GenerateError::Unavailable("mock".into()),
        }
    }
}

/// Deterministic test backend.
pub struct Mock {
    name: &'static str,
    ready: Arc<AtomicBool>,
    config: MockConfig,
}

impl Mock {
    /// Build a `Mock` that reports ready immediately and emits a single canned
    /// token followed by `Done`.
    pub fn new() -> Self {
        Self::with_config(MockConfig {
            tokens: vec!["mock-response".into()],
            ..Default::default()
        })
    }

    /// Build a `Mock` with custom failure-mode configuration.
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            name: "mock",
            ready: Arc::new(AtomicBool::new(true)),
            config,
        }
    }

    /// Toggle the backend's reported readiness. Used by tests of the
    /// listener-gate invariant.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }
}

impl Default for Mock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Backend for Mock {
    fn name(&self) -> &str {
        self.name
    }

    fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    async fn generate(&self, _req: Resolved) -> Result<TokenStream, GenerateError> {
        if let Some(err) = self.config.pre_stream_error {
            return Err(err.into());
        }
        if !self.ready() {
            return Err(GenerateError::NotReady);
        }

        let tokens = self.config.tokens.clone();
        let drop_after = self.config.mid_stream_drop_after;
        let (tx, rx) = tokio::sync::mpsc::channel(8);

        // Spawned so dropping the stream (which drops `rx`) cancels by
        // closing the channel — `tx.send` then returns Err and we exit.
        tokio::spawn(async move {
            let mut completion_tokens: u32 = 0;
            for (emitted, tok) in tokens.into_iter().enumerate() {
                if let Some(n) = drop_after {
                    if emitted >= n {
                        // Simulate mid-stream failure: stop without Done.
                        return;
                    }
                }
                if tx.send(TokenEvent::Token(tok)).await.is_err() {
                    return; // receiver dropped → cancellation
                }
                completion_tokens = completion_tokens.saturating_add(1);
            }
            let _ = tx
                .send(TokenEvent::Done {
                    stop_reason: StopReason::End,
                    usage: Usage {
                        prompt_tokens: 0,
                        completion_tokens,
                    },
                })
                .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}
