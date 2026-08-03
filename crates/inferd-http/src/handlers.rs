//! HTTP routes + handlers for the OpenAI-compat surface.
//!
//! Each request dials a **fresh** daemon client (a cheap UDS/pipe
//! connect) so the daemon's admission queue multiplexes concurrent HTTP
//! requests — `ClientV2` serialises through one connection, so sharing
//! one would bottleneck. Client-disconnect cancellation is inherited:
//! if the HTTP client goes away, the response stream is dropped, which
//! drops the daemon client, which the daemon sees as a disconnect and
//! cancels the in-flight job (ADR 0007).

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use inferd_client::{AdminClient, ClientV2, EmbedClient};
use inferd_openai_wire::{ChatRequest, ContentPart, EmbeddingsRequest, MessageContent};
use inferd_proto::embed::EmbedResponse;
use inferd_proto::v2::ResponseV2;
use tokio_stream::wrappers::ReceiverStream;

use crate::AppState;
use crate::error::HttpError;
use crate::translate::{self, ChunkBuilder};

/// Build the router. When `token` is `Some`, the `/v1/*` API routes
/// require a matching `Authorization: Bearer <token>` header. `/health`
/// is always unauthenticated — it's a liveness probe that exposes
/// nothing sensitive, so monitoring doesn't need the token.
pub fn router(state: AppState, token: Option<String>) -> Router {
    let mut api = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/models", get(models))
        // Explicit inbound body cap (don't rely on axum's implicit
        // default). Bounds pre-daemon memory for a giant messages[]/
        // input[] payload; the daemon separately enforces the 64 MiB
        // frame cap. 8 MiB comfortably covers real chat/embed requests.
        .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(state);

    if let Some(tok) = token {
        api = api.layer(axum::middleware::from_fn(move |headers, req, next| {
            let tok = tok.clone();
            async move { require_bearer(&tok, headers, req, next).await }
        }));
    }

    // /health stays outside the auth layer (unauthenticated liveness).
    api.route("/health", get(health))
}

async fn require_bearer(
    expected: &str,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let ok = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        // Constant-time compare so token verification doesn't leak the
        // token via response timing (`subtle::ConstantTimeEq`). Equal
        // lengths required for ct_eq; unequal lengths are a definite
        // mismatch, so short-circuit there without a timing signal about
        // the content.
        .map(|t| {
            use subtle::ConstantTimeEq;
            t.len() == expected.len() && bool::from(t.as_bytes().ct_eq(expected.as_bytes()))
        })
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        HttpError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_request_error",
            "missing or invalid bearer token",
        )
        .into_response()
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

/// `GET /v1/models` — advertise the single warm model.
async fn models(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "object": "list",
        "data": [{
            "id": state.model_name(),
            "object": "model",
            "owned_by": "inferd",
        }],
    }))
}

/// A monotonic-ish creation timestamp for response objects. Uses the
/// wall clock; if unavailable, 0. (Cosmetic; clients rarely inspect it.)
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn req_id() -> String {
    // A unique-enough id without pulling a uuid dep: timestamp-nanos.
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("chatcmpl-{n:x}")
}

/// `POST /v1/chat/completions`.
async fn chat_completions(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Response {
    let streaming = req.stream;
    let id = req_id();
    let created = now_unix();
    let model = state.model_name().to_string();

    // The bridge must resample audio to the rate the backend requires —
    // the daemon rejects any other rate and never resamples (ADR 0025).
    // That rate is a daemon property read off the admin socket, so it is
    // fetched per-request rather than cached: a daemon restart onto a
    // different mmproj changes it, and a cached value would then produce
    // confidently-wrong audio. Only requests that actually carry audio
    // pay for the extra connect.
    let audio_rate = if request_has_audio(&req) {
        match admin_audio_sample_rate(&state).await {
            Ok(AudioSupport::Rate(r)) => Some(r),
            // The backend takes audio but didn't say at what rate — a
            // pre-v0.6.2 daemon. Distinguishing this from "no audio
            // backend" matters: the two need different fixes, and the
            // wrong message sends the operator hunting the wrong one.
            Ok(AudioSupport::RateUnknown) => {
                return HttpError::bad_request(
                    "the daemon accepts audio but does not advertise the required sample rate; \
                     upgrade the daemon (audio_sample_rate was added in v0.6.2)",
                )
                .into_response();
            }
            Ok(AudioSupport::NoAudio) => None,
            Err(resp) => return resp,
        }
    } else {
        None
    };

    // Translate request.
    let v2 = match translate::chat_request_to_v2(req, id.clone(), audio_rate) {
        Ok(v) => v,
        Err(e) => return HttpError::bad_request(e.to_string()).into_response(),
    };

    // Dial a fresh generation client (retry-and-wait for daemon readiness).
    let mut client = match dial_gen(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    let stream = match client.generate(v2).await {
        Ok(s) => s,
        Err(e) => return HttpError::daemon_unreachable(format!("generate: {e}")).into_response(),
    };

    if streaming {
        chat_stream_response(id, model, created, client, stream)
    } else {
        chat_collect_response(id, model, created, client, stream).await
    }
}

/// Streaming SSE path. Moves the client into the stream so it lives as
/// long as the response (drop-on-disconnect cancels the daemon job).
fn chat_stream_response(
    id: String,
    model: String,
    created: u64,
    client: ClientV2,
    mut frames: inferd_client::FrameStreamV2,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(async move {
        // Keep the client alive for the duration.
        let _client = client;
        let mut builder = ChunkBuilder::new(id, model, created);
        while let Some(item) = frames.next().await {
            match item {
                Ok(frame @ ResponseV2::Frame { .. }) => {
                    if let Some(chunk) = builder.ingest(&frame) {
                        let data = serde_json::to_string(&chunk).unwrap_or_default();
                        if tx.send(Ok(Event::default().data(data))).await.is_err() {
                            return; // client hung up → drop client → daemon cancels
                        }
                    }
                }
                Ok(ResponseV2::Done {
                    ref usage,
                    stop_reason,
                    ..
                }) => {
                    let chunk = builder.finalize(usage, stop_reason);
                    let data = serde_json::to_string(&chunk).unwrap_or_default();
                    let _ = tx.send(Ok(Event::default().data(data))).await;
                    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
                    return;
                }
                Ok(ResponseV2::Error { code, message, .. }) => {
                    // Mid-stream error: emit an OpenAI-shaped error event, then close.
                    let env = inferd_openai_wire::ErrorEnvelope {
                        error: inferd_openai_wire::ErrorBody {
                            message,
                            kind: "api_error".into(),
                            code: Some(format!("{code:?}")),
                        },
                    };
                    let data = serde_json::to_string(&env).unwrap_or_default();
                    let _ = tx.send(Ok(Event::default().data(data))).await;
                    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
                    return;
                }
                Err(e) => {
                    let env = inferd_openai_wire::ErrorEnvelope {
                        error: inferd_openai_wire::ErrorBody {
                            message: format!("stream: {e}"),
                            kind: "api_error".into(),
                            code: None,
                        },
                    };
                    let data = serde_json::to_string(&env).unwrap_or_default();
                    let _ = tx.send(Ok(Event::default().data(data))).await;
                    return;
                }
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).into_response()
}

/// Non-streaming path: collect the whole generation into one JSON body.
async fn chat_collect_response(
    id: String,
    model: String,
    created: u64,
    client: ClientV2,
    mut frames: inferd_client::FrameStreamV2,
) -> Response {
    let _client = client;
    let mut builder = ChunkBuilder::new(id.clone(), model.clone(), created);
    let mut text = String::new();
    loop {
        match frames.next().await {
            Some(Ok(ResponseV2::Frame { ref block, .. })) => {
                if let inferd_proto::v2::ResponseBlock::Text { delta } = block {
                    text.push_str(delta);
                }
                // tool calls buffered in the builder for finish_reason/usage
                let f = ResponseV2::Frame {
                    id: id.clone(),
                    block: block.clone(),
                };
                let _ = builder.ingest(&f);
            }
            Some(Ok(ResponseV2::Done {
                ref usage,
                stop_reason,
                ..
            })) => {
                let finish = translate::stop_reason_to_openai(stop_reason);
                let body = serde_json::json!({
                    "id": id,
                    "object": "chat.completion",
                    "created": created,
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": text },
                        "finish_reason": finish,
                    }],
                    "usage": {
                        "prompt_tokens": usage.input_tokens,
                        "completion_tokens": usage.output_tokens,
                        "total_tokens": usage.input_tokens + usage.output_tokens,
                    }
                });
                let _ = builder; // (tool-call assembly for non-stream is a follow-up; text path complete)
                return (StatusCode::OK, Json(body)).into_response();
            }
            Some(Ok(ResponseV2::Error { code, message, .. })) => {
                return HttpError::from_inferd(code, message).into_response();
            }
            Some(Err(e)) => {
                return HttpError::daemon_unreachable(format!("stream: {e}")).into_response();
            }
            None => {
                return HttpError::daemon_unreachable("daemon closed before terminal frame")
                    .into_response();
            }
        }
    }
}

/// `POST /v1/embeddings`.
async fn embeddings(State(state): State<AppState>, Json(req): Json<EmbeddingsRequest>) -> Response {
    let id = req_id();
    let model = state.model_name().to_string();
    let (embed_req, encoding) = match translate::embeddings_request_to_inferd(req, id) {
        Ok(r) => r,
        Err(e) => return HttpError::bad_request(e.to_string()).into_response(),
    };

    let mut client = match dial_embed(&state).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    match client.embed(embed_req).await {
        Ok(EmbedResponse::Embeddings {
            embeddings, usage, ..
        }) => {
            let resp = translate::embeddings_response_to_openai(
                embeddings,
                model,
                usage.input_tokens,
                encoding,
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Ok(EmbedResponse::Error { message, .. }) => {
            // Embed error codes are their own enum; map generically.
            HttpError::new(StatusCode::BAD_GATEWAY, "api_error", message).into_response()
        }
        Err(e) => HttpError::daemon_unreachable(format!("embed: {e}")).into_response(),
    }
}

// --- daemon dialing (per request) -----------------------------------

async fn dial_gen(state: &AppState) -> Result<ClientV2, Response> {
    let addr = state.gen_addr().0.clone();
    let timeout = state.startup_timeout();
    dial_with_wait(timeout, move || {
        let addr = addr.clone();
        async move { dial_gen_once(&addr).await }
    })
    .await
    .map_err(|e| {
        HttpError::daemon_unreachable(format!("connect generation socket: {e}")).into_response()
    })
}

async fn dial_embed(state: &AppState) -> Result<EmbedClient, Response> {
    let addr = state.embed_addr().0.clone();
    let timeout = state.startup_timeout();
    dial_with_wait(timeout, move || {
        let addr = addr.clone();
        async move { dial_embed_once(&addr).await }
    })
    .await
    .map_err(|e| {
        HttpError::daemon_unreachable(format!("connect embed socket: {e}")).into_response()
    })
}

/// True if any message carries an `input_audio` content part. Cheap
/// structural scan so a text-only request never pays for the admin dial.
fn request_has_audio(req: &ChatRequest) -> bool {
    req.messages.iter().any(|m| match &m.content {
        Some(MessageContent::Parts(parts)) => parts
            .iter()
            .any(|p| matches!(p, ContentPart::InputAudio { .. })),
        _ => false,
    })
}

/// What the daemon's capabilities say about audio input.
///
/// `RateUnknown` is deliberately distinct from `NoAudio`: a daemon older
/// than v0.6.2 advertises `audio: true` with no `audio_sample_rate`,
/// which is an upgrade problem, not a model-capability problem. Folding
/// the two together produces a 400 that names the wrong cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioSupport {
    /// A backend takes audio and requires this rate.
    Rate(u32),
    /// A backend takes audio but advertised no required rate.
    RateUnknown,
    /// No registered backend takes audio.
    NoAudio,
}

impl AudioSupport {
    /// Fold one capabilities frame in. Called once per frame in the caps
    /// prefix; the first backend that advertises a *rate* wins, since a
    /// generation backend and an embed backend can both be registered
    /// and only one has an audio encoder. A rate-less audio backend is
    /// remembered but doesn't end the scan — a later frame may carry the
    /// concrete rate.
    fn fold(self, ev: &inferd_client::AdminEvent) -> Self {
        if matches!(self, Self::Rate(_)) || ev.audio != Some(true) {
            return self;
        }
        match ev.audio_sample_rate {
            Some(r) => Self::Rate(r),
            None => Self::RateUnknown,
        }
    }
}

/// Read the sample rate the daemon's active backend requires for audio
/// attachments, off its admin capabilities frames.
///
/// The daemon writes one capabilities frame per registered backend and
/// then exactly one lifecycle snapshot frame on **every** connect — the
/// caps are retained per backend, so a bridge connecting long after
/// `ready` still sees them. That makes the snapshot frame a deterministic
/// terminator: read until the first non-capabilities frame.
async fn admin_audio_sample_rate(state: &AppState) -> Result<AudioSupport, Response> {
    let addr = state.admin_addr().0.clone();
    let mut admin = dial_admin_once(&addr).await.map_err(|e| {
        HttpError::daemon_unreachable(format!("connect admin socket for audio rate: {e}"))
            .into_response()
    })?;

    // Bounded read: the connect prefix is caps-then-snapshot, and a
    // timeout keeps a wedged daemon from hanging the HTTP request.
    let deadline = Duration::from_secs(5);
    let read = tokio::time::timeout(deadline, async {
        let mut support = AudioSupport::NoAudio;
        loop {
            let ev = admin.recv().await?;
            if ev.status != "capabilities" {
                // Snapshot (or any lifecycle frame) — the caps prefix is done.
                return Ok::<AudioSupport, inferd_client::AdminError>(support);
            }
            support = support.fold(&ev);
        }
    })
    .await;

    match read {
        Ok(Ok(support)) => Ok(support),
        Ok(Err(e)) => Err(
            HttpError::daemon_unreachable(format!("read admin capabilities: {e}")).into_response(),
        ),
        Err(_) => Err(HttpError::daemon_unreachable(
            "timed out reading audio sample rate from the admin socket",
        )
        .into_response()),
    }
}

#[cfg(unix)]
async fn dial_admin_once(addr: &str) -> Result<AdminClient, inferd_client::AdminError> {
    AdminClient::dial_admin_uds(std::path::Path::new(addr)).await
}
#[cfg(windows)]
async fn dial_admin_once(addr: &str) -> Result<AdminClient, inferd_client::AdminError> {
    AdminClient::dial_admin_pipe(addr).await
}

#[cfg(unix)]
async fn dial_gen_once(addr: &str) -> Result<ClientV2, inferd_client::ClientError> {
    ClientV2::dial_uds(std::path::Path::new(addr)).await
}
#[cfg(windows)]
async fn dial_gen_once(addr: &str) -> Result<ClientV2, inferd_client::ClientError> {
    ClientV2::dial_pipe(addr).await
}
#[cfg(unix)]
async fn dial_embed_once(addr: &str) -> Result<EmbedClient, inferd_client::ClientError> {
    EmbedClient::dial_uds(std::path::Path::new(addr)).await
}
#[cfg(windows)]
async fn dial_embed_once(addr: &str) -> Result<EmbedClient, inferd_client::ClientError> {
    EmbedClient::dial_pipe(addr).await
}

/// Retry a dial closure with bounded backoff until it succeeds or the
/// timeout elapses — mirrors `inferd_client::dial_and_wait_ready` but
/// works for both client types via a closure.
async fn dial_with_wait<T, F, Fut>(
    timeout: Duration,
    mut dial: F,
) -> Result<T, inferd_client::ClientError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, inferd_client::ClientError>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut backoff = Duration::from_millis(100);
    loop {
        match dial().await {
            Ok(c) => return Ok(c),
            Err(e) => {
                if tokio::time::Instant::now() >= deadline
                    || !inferd_client::is_transient_dial_error(&e)
                {
                    return Err(e);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fold a sequence of capabilities frames given as real wire JSON —
    /// the same bytes the daemon writes — so these cover the serde shape
    /// as well as the classification.
    fn fold_caps(frames: &[&str]) -> AudioSupport {
        frames.iter().fold(AudioSupport::NoAudio, |acc, json| {
            let ev: inferd_client::AdminEvent =
                serde_json::from_str(json).expect("capabilities frame parses");
            acc.fold(&ev)
        })
    }

    // Verbatim from a live `gemma-4-e4b` daemon on the admin socket.
    const CAPS_AUDIO_WITH_RATE: &str = r#"{"accelerator":"cuda","audio":true,"audio_sample_rate":16000,"backend":"gemma-4-e4b","embed":false,"id":"admin","status":"capabilities","type":"status","v2":true,"vision":true,"wire_version":1}"#;
    // Verbatim from a v0.6.1 GA daemon — `audio_sample_rate` predates it.
    const CAPS_AUDIO_NO_RATE: &str = r#"{"accelerator":"cuda","audio":true,"backend":"gemma-4-e4b","embed":false,"id":"admin","status":"capabilities","type":"status","v2":true,"vision":true,"wire_version":1}"#;
    // Verbatim from an embed-only daemon.
    const CAPS_EMBED_ONLY: &str = r#"{"accelerator":"cuda","audio":false,"backend":"embeddinggemma-300m","embed":true,"id":"admin","status":"capabilities","type":"status","v2":false,"vision":false,"wire_version":1}"#;

    #[test]
    fn audio_capable_backend_yields_its_rate() {
        assert_eq!(
            fold_caps(&[CAPS_AUDIO_WITH_RATE]),
            AudioSupport::Rate(16_000)
        );
    }

    #[test]
    fn embed_only_daemon_yields_no_audio() {
        assert_eq!(fold_caps(&[CAPS_EMBED_ONLY]), AudioSupport::NoAudio);
    }

    #[test]
    fn audio_without_a_rate_is_distinct_from_no_audio() {
        // A pre-v0.6.2 daemon. Reporting this as "does not accept audio
        // input" names the wrong cause and sends the operator looking at
        // their model instead of their daemon version.
        assert_eq!(fold_caps(&[CAPS_AUDIO_NO_RATE]), AudioSupport::RateUnknown);
    }

    #[test]
    fn a_later_frame_can_supply_the_rate() {
        // Two backends registered, the rate-less one first: the scan must
        // keep going rather than latch RateUnknown.
        assert_eq!(
            fold_caps(&[CAPS_AUDIO_NO_RATE, CAPS_AUDIO_WITH_RATE]),
            AudioSupport::Rate(16_000)
        );
    }

    #[test]
    fn a_concrete_rate_is_not_overwritten() {
        assert_eq!(
            fold_caps(&[CAPS_AUDIO_WITH_RATE, CAPS_AUDIO_NO_RATE, CAPS_EMBED_ONLY]),
            AudioSupport::Rate(16_000)
        );
    }
}
