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
use inferd_client::{ClientV2, EmbedClient};
use inferd_openai_wire::{ChatRequest, EmbeddingsRequest};
use inferd_proto::embed::EmbedResponse;
use inferd_proto::v2::ResponseV2;
use tokio_stream::wrappers::ReceiverStream;

use crate::AppState;
use crate::error::HttpError;
use crate::translate::{self, ChunkBuilder};

/// Build the router. When `token` is `Some`, all routes require a
/// matching `Authorization: Bearer <token>` header.
pub fn router(state: AppState, token: Option<String>) -> Router {
    let api = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/models", get(models))
        .route("/health", get(health))
        .with_state(state);

    if let Some(tok) = token {
        api.layer(axum::middleware::from_fn(move |headers, req, next| {
            let tok = tok.clone();
            async move { require_bearer(&tok, headers, req, next).await }
        }))
    } else {
        api
    }
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
        .map(|t| t == expected)
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

    // Translate request.
    let v2 = match translate::chat_request_to_v2(req, id.clone()) {
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
                Ok(ResponseV2::Frame { .. }) => {
                    if let Some(chunk) = builder.ingest(item.as_ref().unwrap()) {
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
