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

use crate::backend::{
    Backend, BackendCapabilities, EmbedError, EmbedResult, GenerateError, TokenEventV2,
    TokenStreamV2,
};
use async_trait::async_trait;
use inferd_proto::embed::{EmbedResolved, EmbedUsage};
use inferd_proto::v2::{ResolvedV2, StopReasonV2, UsageV2};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Optional sleep between emitted tokens, in milliseconds. Used by
    /// the concurrency stress harness to make per-request work
    /// observable so admission queueing actually engages. `None` means
    /// no delay (the historical behaviour).
    pub token_delay_ms: Option<u64>,
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

    /// Mock advertises v2 + thinking + embed so daemon-side dispatch
    /// across all three sockets can be exercised end-to-end without a
    /// real engine. Multimodal / tool flags stay `false` — Mock
    /// doesn't pretend to ingest images or parse tool calls.
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            v2: true,
            thinking: true,
            embed: true,
            ..BackendCapabilities::default()
        }
    }

    /// v2 generation. Same token tape + delays as the prior v1 path but
    /// emits `TokenEventV2::Text(...)` and a v2 `Done` frame with
    /// `StopReasonV2::EndTurn` and `UsageV2` field names. Mid-stream
    /// drop and pre-stream error knobs apply identically.
    async fn generate_v2(&self, _req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        if let Some(err) = self.config.pre_stream_error {
            return Err(err.into());
        }
        if !self.ready() {
            return Err(GenerateError::NotReady);
        }

        let tokens = self.config.tokens.clone();
        let drop_after = self.config.mid_stream_drop_after;
        let token_delay = self
            .config
            .token_delay_ms
            .map(std::time::Duration::from_millis);
        let (tx, rx) = tokio::sync::mpsc::channel(8);

        tokio::spawn(async move {
            let mut output_tokens: u32 = 0;
            for (emitted, tok) in tokens.into_iter().enumerate() {
                if let Some(n) = drop_after
                    && emitted >= n
                {
                    return;
                }
                if let Some(d) = token_delay {
                    tokio::time::sleep(d).await;
                }
                if tx.send(TokenEventV2::Text(tok)).await.is_err() {
                    return;
                }
                output_tokens = output_tokens.saturating_add(1);
            }
            let _ = tx
                .send(TokenEventV2::Done {
                    stop_reason: StopReasonV2::EndTurn,
                    usage: UsageV2 {
                        input_tokens: 0,
                        output_tokens,
                    },
                })
                .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Deterministic mock embedding. Emits one fixed-length vector per
    /// input string; entries are derived from the input length so
    /// tests can assert correlation. `dimensions` defaults to 8 when
    /// the request doesn't supply one; otherwise the request's value
    /// is honoured (no model-specific MRL set to validate against).
    /// Pre-stream-error knob (`MockError::Unavailable` /
    /// `InvalidRequest`) is reused on the embed path to exercise
    /// daemon-side error mapping; `NotReady` mode short-circuits to
    /// `EmbedError::NotReady` to mirror the v1/v2 paths.
    async fn embed(&self, req: EmbedResolved) -> Result<EmbedResult, EmbedError> {
        if let Some(err) = self.config.pre_stream_error {
            return Err(match err {
                MockError::NotReady => EmbedError::NotReady,
                MockError::InvalidRequest => EmbedError::InvalidRequest("mock".into()),
                MockError::Unavailable => EmbedError::Unavailable("mock".into()),
            });
        }
        if !self.ready() {
            return Err(EmbedError::NotReady);
        }

        let dimensions = req.dimensions.unwrap_or(8);
        let mut input_tokens: u32 = 0;
        let embeddings = req
            .input
            .iter()
            .map(|s| {
                input_tokens = input_tokens.saturating_add(s.len() as u32);
                let len_f = s.len() as f32;
                (0..dimensions)
                    .map(|i| (i as f32 + 1.0) / (len_f + 1.0))
                    .collect()
            })
            .collect();

        Ok(EmbedResult {
            embeddings,
            dimensions,
            model: "mock".into(),
            usage: EmbedUsage { input_tokens },
        })
    }
}
