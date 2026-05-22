//! Embed connection lifecycle — Phase 6B-7 part 4.
//!
//! Per ADR 0017, embeddings live on a *separate* socket from v1 and
//! v2. This module mirrors `lifecycle_v2.rs` but for the embed wire
//! types (`inferd_proto::embed::EmbedRequest` / `EmbedResponse`).
//!
//! Single-frame request / single-frame response — embeddings are not
//! streamed (the result is a complete vector, there is nothing to
//! stream). The connection stays open for the next request.
//!
//! Per request:
//!   1. Read one NDJSON frame, parse as `EmbedRequest`.
//!   2. `EmbedRequest::resolve()` — structural validation.
//!   3. Admission gate (same Admission shared with v1 / v2; one slot
//!      is one slot regardless of wire surface).
//!   4. Dispatch through the router; check the chosen backend's
//!      `capabilities().embed` flag — backends that don't support
//!      embeddings yield `Error{EmbedUnsupported, ...}`.
//!   5. `backend.embed(resolved)` — errors map to embed error codes.
//!   6. Emit a single `EmbedResponse::Embeddings` or
//!      `EmbedResponse::Error` frame, then loop for the next request.

use crate::auth::{AuthFrame, key_matches};
use crate::endpoint::Connection;
use crate::peercred::PeerIdentity;
use crate::queue::SubmitError;
use crate::router::{Router, RouterError};
use inferd_engine::EmbedError;
use inferd_proto::ProtoError;
use inferd_proto::embed::{EmbedErrorCode, EmbedRequest, EmbedResponse};
use inferd_proto::write_frame;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Per-accept context for embed connections. Reuses v1's
/// `AcceptContext` shape — same TCP API key, same admission gate.
pub use crate::lifecycle::AcceptContext;

/// Handle one accepted embed client connection.
pub async fn handle_embed_connection<C: Connection + 'static>(
    mut conn: C,
    router: Arc<Router>,
    peer: PeerIdentity,
    ctx: AcceptContext,
) -> Result<(), io::Error> {
    let transport = conn.transport();
    info!(
        target: "inferd_daemon::activity",
        transport = transport,
        wire_version = "embed",
        peer = %peer,
        peer_uid = peer.uid,
        peer_pid = peer.pid,
        peer_sid = peer.sid.as_deref(),
        "embed_connection_accepted"
    );

    let (read_half, write_half) = tokio::io::split(&mut conn);
    let mut reader = BufReader::with_capacity(64 * 1024, read_half);
    let writer = Arc::new(Mutex::new(write_half));

    // F-8 first-frame auth on TCP, identical to v1 / v2.
    if transport == "tcp"
        && let Some(expected) = ctx.expected_api_key.as_deref()
    {
        match read_auth_frame(&mut reader).await {
            Some(frame) if key_matches(&frame.key, expected) => {
                debug!(transport, "embed tcp auth ok");
            }
            _ => {
                warn!(
                    target: "inferd_daemon::activity",
                    peer = %peer,
                    wire_version = "embed",
                    "embed_tcp_auth_rejected"
                );
                return Ok(());
            }
        }
    }

    loop {
        let request: EmbedRequest = match read_request_embed(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                let resp = EmbedResponse::Error {
                    id: String::new(),
                    code: error_code_for(&e),
                    message: e.to_string(),
                };
                write_response_embed(&writer, &resp).await?;
                return Ok(());
            }
        };

        let id = request.id.clone();
        let resolved = match request.resolve() {
            Ok(r) => r,
            Err(e) => {
                let resp = EmbedResponse::Error {
                    id,
                    code: EmbedErrorCode::InvalidRequest,
                    message: e.to_string(),
                };
                write_response_embed(&writer, &resp).await?;
                continue;
            }
        };

        // Admission gate. Embed shares the same admission instance as
        // v1 / v2 — one slot is one slot.
        let _admit_permit = match ctx.admission.as_ref().map(|a| a.try_admit()) {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err(SubmitError::QueueFull)) => {
                let resp = EmbedResponse::Error {
                    id: resolved.id.clone(),
                    code: EmbedErrorCode::QueueFull,
                    message: "queue full".into(),
                };
                write_response_embed(&writer, &resp).await?;
                continue;
            }
            Some(Err(SubmitError::Closed)) => {
                let resp = EmbedResponse::Error {
                    id: resolved.id.clone(),
                    code: EmbedErrorCode::BackendUnavailable,
                    message: "admission closed".into(),
                };
                write_response_embed(&writer, &resp).await?;
                return Ok(());
            }
        };

        // Dispatch through the router. `dispatch_embed` filters out
        // slots whose backend doesn't advertise `capabilities().embed`
        // so multi-backend configs that put a generate-only backend
        // ahead of an embed-capable one route embed requests correctly.
        let dispatch = match router.dispatch_embed() {
            Ok(d) => d,
            Err(RouterError::NoBackends) | Err(RouterError::NoneAvailable) => {
                let resp = EmbedResponse::Error {
                    id: resolved.id.clone(),
                    code: EmbedErrorCode::BackendUnavailable,
                    message: "no embed-capable backend available".into(),
                };
                write_response_embed(&writer, &resp).await?;
                continue;
            }
        };
        let backend_name = dispatch.name.clone();
        let backend = dispatch.backend;

        let req_id = resolved.id.clone();
        let n_inputs = resolved.input.len();

        let result = backend.embed(resolved).await;
        match result {
            Ok(out) => {
                let usage = out.usage;
                let dimensions = out.dimensions;
                let frame = EmbedResponse::Embeddings {
                    id: req_id.clone(),
                    embeddings: out.embeddings,
                    dimensions,
                    model: out.model,
                    usage,
                    backend: backend_name.clone(),
                };
                write_response_embed(&writer, &frame).await?;
                router.record_success(&backend_name);
                info!(
                    target: "inferd_daemon::activity",
                    req_id = %req_id,
                    backend = %backend_name,
                    wire_version = "embed",
                    n_inputs = n_inputs,
                    input_tokens = usage.input_tokens,
                    dimensions = dimensions,
                    "embed_request_done"
                );
            }
            Err(e) => {
                let (code, message, is_backend_failure) = match e {
                    EmbedError::InvalidRequest(m) => (EmbedErrorCode::InvalidRequest, m, false),
                    EmbedError::NotReady => (
                        EmbedErrorCode::BackendUnavailable,
                        "backend not ready".into(),
                        true,
                    ),
                    EmbedError::Unavailable(m) => (EmbedErrorCode::BackendUnavailable, m, true),
                    EmbedError::Unsupported => (
                        EmbedErrorCode::EmbedUnsupported,
                        "embed not supported by this backend".into(),
                        false,
                    ),
                    EmbedError::Internal(m) => (EmbedErrorCode::Internal, m, true),
                };
                if is_backend_failure {
                    router.record_failure(&backend_name);
                }
                let frame = EmbedResponse::Error {
                    id: req_id,
                    code,
                    message,
                };
                write_response_embed(&writer, &frame).await?;
            }
        }
    }
}

fn error_code_for(e: &ProtoError) -> EmbedErrorCode {
    match e {
        ProtoError::FrameTooLarge => EmbedErrorCode::FrameTooLarge,
        ProtoError::Decode(_) | ProtoError::InvalidRequest(_) => EmbedErrorCode::InvalidRequest,
        ProtoError::Io(_) => EmbedErrorCode::Internal,
    }
}

async fn read_auth_frame<R>(reader: &mut R) -> Option<AuthFrame>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    let mut line = Vec::with_capacity(256);
    let limit = inferd_proto::MAX_FRAME_BYTES;
    loop {
        let buf = reader.fill_buf().await.ok()?;
        if buf.is_empty() {
            return None;
        }
        if let Some(idx) = buf.iter().position(|&b| b == b'\n') {
            if line.len() + idx > limit {
                return None;
            }
            line.extend_from_slice(&buf[..idx]);
            reader.consume(idx + 1);
            return AuthFrame::from_json(&line);
        }
        if line.len() + buf.len() > limit {
            return None;
        }
        line.extend_from_slice(buf);
        let n = buf.len();
        reader.consume(n);
    }
}

async fn read_request_embed<R>(reader: &mut R) -> Result<Option<EmbedRequest>, ProtoError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    let mut line = Vec::with_capacity(512);
    let limit = inferd_proto::MAX_FRAME_BYTES;
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return inferd_proto::read_frame::<&[u8], EmbedRequest>(&mut &line[..]);
        }
        if let Some(idx) = buf.iter().position(|&b| b == b'\n') {
            if line.len() + idx > limit {
                return Err(ProtoError::FrameTooLarge);
            }
            line.extend_from_slice(&buf[..=idx]);
            reader.consume(idx + 1);
            return inferd_proto::read_frame::<&[u8], EmbedRequest>(&mut &line[..]);
        }
        if line.len() + buf.len() > limit {
            return Err(ProtoError::FrameTooLarge);
        }
        line.extend_from_slice(buf);
        let n = buf.len();
        reader.consume(n);
    }
}

async fn write_response_embed<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &EmbedResponse,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise embed response: {e}")))?;
    let mut guard = writer.lock().await;
    guard.write_all(&buf).await?;
    guard.flush().await?;
    Ok(())
}

/// Serve an embed TCP listener.
pub async fn serve_tcp_embed(
    listener: tokio::net::TcpListener,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!(addr = ?listener.local_addr()?, "embed tcp listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("embed tcp shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, peer_addr) = accept?;
                let peer = PeerIdentity::from_tcp(peer_addr);
                let r = Arc::clone(&router);
                let ctx = ctx.clone();
                debug!(?peer_addr, "embed tcp accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_embed_connection(stream, r, peer, ctx).await {
                        warn!(error = ?e, "embed connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve an embed Unix domain socket listener.
#[cfg(unix)]
pub async fn serve_uds_embed(
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!("embed uds listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("embed uds shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let r = Arc::clone(&router);
                let peer = crate::peercred::unix::from_stream(&stream)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "embed SO_PEERCRED failed; recording empty unix identity");
                        crate::peercred::PeerIdentity {
                            uid: None, gid: None, pid: None,
                            sid: None, remote_addr: None,
                            transport: "unix",
                        }
                    });
                let ctx = ctx.clone();
                debug!(?peer, "embed uds accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_embed_connection(stream, r, peer, ctx).await {
                        warn!(error = ?e, "embed connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve an embed Windows named pipe listener.
#[cfg(windows)]
pub async fn serve_named_pipe_embed(
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    use crate::endpoint::bind_named_pipe;

    info!(path = %path, "embed named pipe listener accepting");
    let mut server = first_instance;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("embed named pipe shutdown signalled");
                return Ok(());
            }
            connect_result = server.connect() => {
                connect_result?;
                let connected = server;
                server = bind_named_pipe(path, false)?;

                let peer = crate::peercred::windows::from_stream(&connected)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "embed GetNamedPipeClientProcessId failed; empty pipe identity");
                        crate::peercred::PeerIdentity {
                            uid: None, gid: None, pid: None,
                            sid: None, remote_addr: None,
                            transport: "pipe",
                        }
                    });
                let r = Arc::clone(&router);
                let ctx = ctx.clone();
                debug!(?peer, "embed named pipe accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_embed_connection(connected, r, peer, ctx).await {
                        warn!(error = ?e, "embed connection terminated with error");
                    }
                });
            }
        }
    }
}
