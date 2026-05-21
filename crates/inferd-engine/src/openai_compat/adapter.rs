//! `OpenAiCompat` — `Backend` implementation that talks the OpenAI
//! Chat Completions wire to any compatible upstream (OpenAI itself,
//! vLLM, LM Studio, LocalAI, llama.cpp's `server`, OpenRouter, …).
//!
//! This is the v0.2 cloud-adapter carve-out described in ADR 0006:
//! the daemon never *serves* HTTP, but its `Backend` trait can be
//! implemented by an adapter that reaches out via HTTPS. The adapter
//! is feature-gated (`openai`) so consumers who only want the trait
//! don't pull in `reqwest`.

use super::client::{ChatChunk, ChatRequest};
use super::mapper::{ChunkAccumulator, MapperError, request_from_resolved};
use crate::backend::{
    AcceleratorInfo, Backend, BackendCapabilities, GenerateError, TokenEventV2, TokenStream,
    TokenStreamV2,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use inferd_proto::Resolved;
use inferd_proto::v2::ResolvedV2;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

/// Configuration for `OpenAiCompat`.
#[derive(Debug, Clone)]
pub struct OpenAiCompatConfig {
    /// Base URL of the upstream (no trailing slash, no path). The
    /// adapter appends `/v1/chat/completions`. Examples:
    /// `https://api.openai.com`, `http://localhost:11434`,
    /// `https://openrouter.ai`.
    pub base_url: String,
    /// Bearer token sent as `Authorization: Bearer <token>`. Empty
    /// string skips the header (some self-hosted endpoints accept
    /// unauthenticated traffic).
    pub api_key: String,
    /// Upstream model identifier echoed in `model` on the wire.
    pub model: String,
    /// Total request timeout. The default is 5 minutes — long enough
    /// for a slow first-token from a cold cloud model, short enough
    /// to surface stuck requests rather than hang forever.
    pub timeout: Duration,
}

impl Default for OpenAiCompatConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            timeout: Duration::from_secs(300),
        }
    }
}

/// Errors specific to the `OpenAiCompat` adapter. Most surface as
/// `GenerateError::Unavailable` — the daemon translates that to
/// `BackendUnavailable` per ADR 0007 §"failure semantics".
#[derive(Debug, thiserror::Error)]
pub enum OpenAiCompatError {
    /// reqwest-level transport error (DNS, TLS, connection refused, …).
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    /// Upstream returned a non-2xx HTTP status before the SSE stream
    /// opened.
    #[error("upstream HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP status code from the upstream.
        status: u16,
        /// Response body (truncated to the first 4 KB).
        body: String,
    },
    /// JSON deserialise failed on a `data:` line. The adapter emits
    /// the line for debug; production deployments shouldn't see this
    /// unless the upstream's wire is non-conformant.
    #[error("malformed SSE chunk: {0}")]
    MalformedChunk(String),
    /// Mapper rejected the request before we hit the wire.
    #[error("request mapping: {0}")]
    Mapper(#[from] MapperError),
}

impl From<OpenAiCompatError> for GenerateError {
    fn from(e: OpenAiCompatError) -> Self {
        match e {
            OpenAiCompatError::Mapper(MapperError::AttachmentUnsupported(_)) => {
                GenerateError::InvalidRequest(e.to_string())
            }
            OpenAiCompatError::Mapper(MapperError::UnknownContentBlock) => {
                GenerateError::InvalidRequest(e.to_string())
            }
            OpenAiCompatError::Mapper(MapperError::NonTextToolResult) => {
                GenerateError::InvalidRequest(e.to_string())
            }
            _ => GenerateError::Unavailable(e.to_string()),
        }
    }
}

/// `Backend` adapter for OpenAI Chat Completions and compatible APIs.
pub struct OpenAiCompat {
    name: &'static str,
    config: OpenAiCompatConfig,
    client: reqwest::Client,
}

impl OpenAiCompat {
    /// Construct a new adapter. Builds the `reqwest::Client` with
    /// rustls + the configured timeout.
    pub fn new(config: OpenAiCompatConfig) -> Result<Self, OpenAiCompatError> {
        let client = reqwest::Client::builder().timeout(config.timeout).build()?;
        Ok(Self {
            name: "openai-compat",
            config,
            client,
        })
    }

    fn endpoint(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        format!("{base}/v1/chat/completions")
    }

    fn build_request(&self, body: &ChatRequest) -> reqwest::RequestBuilder {
        let mut rb = self
            .client
            .post(self.endpoint())
            .header(CONTENT_TYPE, "application/json")
            .json(body);
        if !self.config.api_key.is_empty() {
            rb = rb.header(
                AUTHORIZATION,
                format!("Bearer {}", self.config.api_key.as_str()),
            );
        }
        rb
    }
}

#[async_trait]
impl Backend for OpenAiCompat {
    fn name(&self) -> &str {
        self.name
    }

    fn ready(&self) -> bool {
        // Stateless adapter — readiness is a transport-time check,
        // not a startup gate. We always claim ready; the daemon's
        // listener gate fires on the trait's `ready()` and we have
        // nothing to load synchronously.
        true
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            v2: true,
            tools: true,
            // Multimodal + thinking deliberately off — see
            // openai_compat/mod.rs for the rationale.
            vision: false,
            audio: false,
            video: false,
            thinking: false,
            // Hardware acceleration is upstream-side and not
            // introspectable from here. Default `Cpu / 0` is the
            // honest answer about what *this process* contributes.
            accelerator: AcceleratorInfo::default(),
        }
    }

    /// v1 path: not supported. v1's flat-string `Resolved` doesn't
    /// carry tool definitions, and the OpenAI surface doesn't add
    /// value over llamacpp for plain text without tools. Consumers
    /// who want this adapter use the v2 socket.
    async fn generate(&self, _req: Resolved) -> Result<TokenStream, GenerateError> {
        Err(GenerateError::Internal(
            "openai-compat backend supports v2 only; use the v2 socket".into(),
        ))
    }

    async fn generate_v2(&self, req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        let body =
            request_from_resolved(&req, &self.config.model).map_err(OpenAiCompatError::from)?;
        let request = self.build_request(&body);

        // Pre-stream errors (transport, non-2xx) surface from
        // generate_v2 as GenerateError; mid-stream errors terminate
        // the stream silently and the daemon translates per ADR 0007.
        let response = request.send().await.map_err(OpenAiCompatError::from)?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".into());
            let truncated = if body.len() > 4096 {
                body[..4096].to_string()
            } else {
                body
            };
            return Err(OpenAiCompatError::HttpStatus {
                status,
                body: truncated,
            }
            .into());
        }

        // Spawn an async task that drives the SSE stream and pushes
        // events onto an mpsc channel; return the receiver as the
        // TokenStreamV2. Dropping the stream drops the receiver,
        // which causes the next `tx.send` to error and terminates
        // the task — matching the cancellation contract.
        let (tx, rx) = mpsc::channel(8);
        let event_stream = response.bytes_stream().eventsource();

        tokio::spawn(async move {
            drive_sse(event_stream, tx).await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn stop(&self, _timeout: Duration) -> Result<(), GenerateError> {
        // Nothing to do — the reqwest::Client drops with the adapter.
        Ok(())
    }
}

/// Drive the SSE stream end-to-end: parse `data:` lines as
/// `ChatChunk`, feed the accumulator, push events out the channel.
/// Terminates on `[DONE]`, on stream end, or when the receiver drops.
async fn drive_sse<S>(mut event_stream: S, tx: mpsc::Sender<TokenEventV2>)
where
    S: futures_util::Stream<
            Item = Result<
                eventsource_stream::Event,
                eventsource_stream::EventStreamError<reqwest::Error>,
            >,
        > + Unpin,
{
    let mut acc = ChunkAccumulator::new();

    while let Some(event) = event_stream.next().await {
        let event = match event {
            Ok(ev) => ev,
            Err(e) => {
                warn!(error = %e, "openai-compat SSE transport error");
                // Bail without finalizing — leaves the stream
                // terminated without a Done frame, which the daemon
                // translates to BackendUnavailable per ADR 0007.
                return;
            }
        };

        // OpenAI signals end of stream with `data: [DONE]`.
        if event.data == "[DONE]" {
            break;
        }

        let chunk: ChatChunk = match serde_json::from_str(&event.data) {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    error = %e,
                    data = %truncate(&event.data, 256),
                    "openai-compat malformed chunk; dropping"
                );
                continue;
            }
        };

        for ev in acc.ingest(chunk) {
            if tx.send(ev).await.is_err() {
                debug!("openai-compat generation cancelled (receiver dropped)");
                return;
            }
        }
    }

    // Stream ended cleanly (or hit [DONE]). Drain any buffered
    // tool-call deltas and emit the terminal Done.
    for ev in acc.finalize() {
        if tx.send(ev).await.is_err() {
            return;
        }
    }
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}
