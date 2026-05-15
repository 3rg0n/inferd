//! Daemon lifecycle: boot → wait-for-ready → bind listener → accept →
//! dispatch → shutdown.
//!
//! The M1 lifecycle wires:
//! - `lock` — single-instance lock at startup (THREAT_MODEL F-2).
//! - `router` — backend selection (no-op v0.1 — picks the only one).
//! - `endpoint` — listener bound only after `router.all_ready()`
//!   (THREAT_MODEL F-13).
//! - `queue` — admission gate (`SubmitError::QueueFull` → wire
//!   `code: queue_full`).
//! - `inferd-proto` — frame parsing and serialisation.
//!
//! Cancellation: dropping a connection drops the in-flight `TokenStream`,
//! which closes the engine's `tx` and stops the spawned generation task.
//! Per ADR 0007 the daemon emits no terminal frame on cancel — the EOF
//! is the signal.

use crate::endpoint::Connection;
use crate::router::{Router, RouterError};
use inferd_engine::{GenerateError, TokenEvent};
use inferd_proto::{write_frame, ErrorCode, ProtoError, Request, Response};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

/// Wait until every backend in `router` reports ready, polling at 50ms
/// intervals up to `timeout`. Returns the duration spent waiting.
///
/// THREAT_MODEL F-13: nothing else creates listeners until this returns.
pub async fn wait_for_ready(router: &Router, timeout: Duration) -> Result<Duration, ReadyTimeout> {
    let started = Instant::now();
    let poll = Duration::from_millis(50);
    loop {
        if router.all_ready() {
            return Ok(started.elapsed());
        }
        if started.elapsed() >= timeout {
            return Err(ReadyTimeout(timeout));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Returned when `wait_for_ready` exhausts its budget without seeing
/// readiness across every backend.
#[derive(Debug, thiserror::Error)]
#[error("backend not ready within {0:?}")]
pub struct ReadyTimeout(pub Duration);

/// Handle one accepted client connection: read framed `Request`s and write
/// framed `Response`s until EOF or fatal error.
///
/// Per request:
/// 1. Read one frame (`read_frame`).
/// 2. `Request::resolve()` — apply defaults, validate. Failures → `error`
///    frame with `code: invalid_request`.
/// 3. `router.dispatch()` — pick a backend.
/// 4. `backend.generate()` — pre-stream errors → `error` frame with
///    `code: backend_unavailable`.
/// 5. Stream `TokenEvent`s, translating each to `Response::Token` /
///    `Response::Done`. If the engine drops the stream without `Done`,
///    emit `error` with `code: backend_unavailable`.
pub async fn handle_connection<C: Connection + 'static>(
    mut conn: C,
    router: Arc<Router>,
) -> Result<(), io::Error> {
    let transport = conn.transport();
    debug!(transport, "connection accepted");

    // Split read and write halves so the generation task can write tokens
    // while we keep reading the next request. We don't actually pipeline
    // requests in M1 (admission queue is 1-active anyway), but the split
    // is needed because tokio AsyncWrite is consumed by `write_all`.
    let (read_half, write_half) = tokio::io::split(&mut conn);
    let mut reader = BufReader::with_capacity(64 * 1024, read_half);
    let writer = Arc::new(Mutex::new(write_half));

    loop {
        // Read one request frame. `read_frame` is sync over a sync BufRead;
        // we have an async reader, so do a small async-to-sync bridge by
        // first reading into a vec and then parsing.
        let request: Request = match read_frame_async(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()), // peer closed cleanly
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                let resp = Response::Error {
                    id: String::new(),
                    code: e.to_error_code(),
                    message: e.to_string(),
                };
                write_response(&writer, &resp).await?;
                return Ok(());
            }
        };

        // Resolve: defaults + validation.
        let id = request.id.clone();
        let resolved = match request.resolve() {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::Error {
                    id,
                    code: ErrorCode::InvalidRequest,
                    message: e.to_string(),
                };
                write_response(&writer, &resp).await?;
                continue;
            }
        };

        // Dispatch through the router.
        let backend = match router.dispatch() {
            Ok(b) => b,
            Err(RouterError::NoBackends) | Err(RouterError::NoneAvailable) => {
                let resp = Response::Error {
                    id: resolved.id.clone(),
                    code: ErrorCode::BackendUnavailable,
                    message: "no backend available".into(),
                };
                write_response(&writer, &resp).await?;
                continue;
            }
        };

        let backend_name = backend.name().to_string();
        let req_id = resolved.id.clone();

        // Generate.
        let mut stream = match backend.generate(resolved).await {
            Ok(s) => s,
            Err(e) => {
                let (code, message) = match e {
                    GenerateError::InvalidRequest(m) => (ErrorCode::InvalidRequest, m),
                    GenerateError::NotReady => {
                        (ErrorCode::BackendUnavailable, "backend not ready".into())
                    }
                    GenerateError::Unavailable(m) => (ErrorCode::BackendUnavailable, m),
                    GenerateError::Internal(m) => (ErrorCode::Internal, m),
                };
                let resp = Response::Error {
                    id: req_id,
                    code,
                    message,
                };
                write_response(&writer, &resp).await?;
                continue;
            }
        };

        // Stream tokens. Build the full content for Response::Done in one
        // pass; the engine reports usage so we don't have to count.
        let mut full = String::new();
        let mut terminal_emitted = false;
        while let Some(ev) = stream.next().await {
            match ev {
                TokenEvent::Token(text) => {
                    let frame = Response::Token {
                        id: req_id.clone(),
                        content: text.clone(),
                    };
                    write_response(&writer, &frame).await?;
                    full.push_str(&text);
                }
                TokenEvent::Done { stop_reason, usage } => {
                    let frame = Response::Done {
                        id: req_id.clone(),
                        content: std::mem::take(&mut full),
                        usage,
                        stop_reason,
                        backend: backend_name.clone(),
                    };
                    write_response(&writer, &frame).await?;
                    terminal_emitted = true;
                    break;
                }
            }
        }

        if !terminal_emitted {
            // Mid-stream backend failure (no Done event). Report and move
            // to next request on the same connection.
            warn!(req_id, backend = %backend_name, "stream ended without done");
            let frame = Response::Error {
                id: req_id,
                code: ErrorCode::BackendUnavailable,
                message: "backend ended stream without terminal frame".into(),
            };
            write_response(&writer, &frame).await?;
        }
    }
}

/// Async wrapper around `inferd_proto::read_frame` for tokio readers.
///
/// Reads bytes asynchronously into a buffer until newline or EOF, then
/// reuses the proto crate's parse path. Honours the 64 MiB cap by deferring
/// to `read_frame` once the line is in memory; we cap our own pre-buffer at
/// the same `MAX_FRAME_BYTES`.
async fn read_frame_async<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Request>, ProtoError> {
    use tokio::io::AsyncBufReadExt;
    let mut br = tokio::io::BufReader::new(reader);
    let mut line = Vec::with_capacity(512);
    let limit = inferd_proto::MAX_FRAME_BYTES;
    loop {
        let buf = br.fill_buf().await?;
        if buf.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            // Trailing line without newline. Defer to the proto crate's
            // sync reader, which handles trailing-line-without-newline as
            // a final frame.
            return inferd_proto::read_frame::<&[u8], Request>(&mut &line[..]);
        }
        if let Some(idx) = buf.iter().position(|&b| b == b'\n') {
            if line.len() + idx > limit {
                return Err(ProtoError::FrameTooLarge);
            }
            line.extend_from_slice(&buf[..=idx]);
            br.consume(idx + 1);
            return inferd_proto::read_frame::<&[u8], Request>(&mut &line[..]);
        }
        if line.len() + buf.len() > limit {
            return Err(ProtoError::FrameTooLarge);
        }
        line.extend_from_slice(buf);
        let n = buf.len();
        br.consume(n);
    }
}

async fn write_response<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &Response,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise response: {e}")))?;
    let mut guard = writer.lock().await;
    guard.write_all(&buf).await?;
    guard.flush().await?;
    Ok(())
}

/// Serve a TCP listener: accept loop, spawn one task per connection.
///
/// Returns when `shutdown` resolves (e.g. a Ctrl-C signal). All in-flight
/// connections are dropped at that point — clients see EOF and treat it as
/// a non-terminal-frame error per `docs/protocol-v1.md`.
pub async fn serve_tcp(
    listener: tokio::net::TcpListener,
    router: Arc<Router>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!(addr = ?listener.local_addr()?, "tcp listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, peer) = accept?;
                let r = Arc::clone(&router);
                debug!(?peer, "tcp accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, r).await {
                        warn!(error = ?e, "connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve a Unix domain socket listener (Unix only).
#[cfg(unix)]
pub async fn serve_uds(
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!("uds listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let r = Arc::clone(&router);
                debug!("uds accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, r).await {
                        warn!(error = ?e, "connection terminated with error");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_engine::mock::Mock;

    #[tokio::test]
    async fn wait_for_ready_returns_when_already_ready() {
        let router = Router::new(vec![Arc::new(Mock::new())]);
        let elapsed = wait_for_ready(&router, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(elapsed < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_not_ready() {
        let mock = Arc::new(Mock::new());
        mock.set_ready(false);
        let router = Router::new(vec![mock]);
        let err = wait_for_ready(&router, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not ready"));
    }

    #[tokio::test]
    async fn wait_for_ready_succeeds_after_delayed_ready() {
        let mock = Arc::new(Mock::new());
        mock.set_ready(false);
        let router = Router::new(vec![mock.clone()]);

        let m2 = Arc::clone(&mock);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            m2.set_ready(true);
        });

        let elapsed = wait_for_ready(&router, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(elapsed >= Duration::from_millis(100));
    }
}
