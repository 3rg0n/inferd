//! Rerank connection lifecycle (ADR 0027).
//!
//! Per ADR 0027, reranking lives on a *fourth* socket, separate from
//! generation and embed. This module mirrors `lifecycle_embed.rs` for the
//! rerank wire types (`inferd_proto::rerank::RerankRequest` /
//! `RerankResponse`) — every framing decision is inherited from ADR 0017
//! unchanged: NDJSON, single-frame request, single-frame response, 64 MiB
//! cap, long-lived connection.
//!
//! Per request:
//!   1. Read one NDJSON frame, parse as `RerankRequest`.
//!   2. `RerankRequest::resolve()` — structural validation, including the
//!      document-count and total-byte bounds. Those bounds matter more
//!      here than on any other surface: rerank is the one surface whose
//!      cost is `O(documents)` *forward passes*, so a cheap frame would
//!      otherwise entitle the sender to unbounded expense while holding
//!      the shared admission permit (the F-1 amplification class).
//!   3. Admission gate (the same `Admission` shared with generation and
//!      embed; one slot is one slot regardless of wire surface).
//!   4. Dispatch through the router on `capabilities().rerank`.
//!   5. `backend.rerank(resolved)` — errors map to rerank error codes.
//!   6. Emit a single `RerankResponse::Rerank` or `RerankResponse::Error`
//!      frame, then loop for the next request.

use crate::endpoint::Connection;
use crate::peercred::PeerIdentity;
use crate::queue::SubmitError;
use crate::router::{Router, RouterError};
use inferd_engine::RerankError;
use inferd_proto::ProtoError;
use inferd_proto::rerank::{RerankErrorCode, RerankRequest, RerankResponse};
use inferd_proto::write_frame;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Per-accept context for rerank connections. Same shape as every other
/// surface — same admission gate, same write timeout.
pub use crate::lifecycle::AcceptContext;

/// Handle one accepted rerank client connection.
pub async fn handle_rerank_connection<C: Connection + 'static>(
    mut conn: C,
    router: Arc<Router>,
    peer: PeerIdentity,
    ctx: AcceptContext,
) -> Result<(), io::Error> {
    let transport = conn.transport();
    info!(
        target: "inferd_daemon::activity",
        transport = transport,
        wire_version = "rerank",
        peer = %peer,
        peer_uid = peer.uid,
        peer_pid = peer.pid,
        peer_sid = peer.sid.as_deref(),
        "rerank_connection_accepted"
    );

    let (read_half, write_half) = tokio::io::split(&mut conn);
    let mut reader = BufReader::with_capacity(64 * 1024, read_half);
    let writer = Arc::new(Mutex::new(write_half));
    // THREAT_MODEL F-17: bounded so a peer that stops reading can't hold
    // the shared admission permit indefinitely.
    let write_timeout = ctx.write_timeout;

    loop {
        let request: RerankRequest = match read_request_rerank(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                let resp = RerankResponse::Error {
                    id: String::new(),
                    code: error_code_for(&e),
                    message: e.to_string(),
                };
                write_response_rerank(&writer, &resp, write_timeout).await?;
                return Ok(());
            }
        };

        let id = request.id.clone();
        let resolved = match request.resolve() {
            Ok(r) => r,
            Err(e) => {
                let resp = RerankResponse::Error {
                    id,
                    code: RerankErrorCode::InvalidRequest,
                    message: e.to_string(),
                };
                write_response_rerank(&writer, &resp, write_timeout).await?;
                continue;
            }
        };

        // Admission gate. Rerank shares the same admission instance as
        // generation and embed — one slot is one slot.
        let _admit_permit = match ctx.admission.as_ref().map(|a| a.try_admit()) {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err(SubmitError::QueueFull)) => {
                let resp = RerankResponse::Error {
                    id: resolved.id.clone(),
                    code: RerankErrorCode::QueueFull,
                    message: "queue full".into(),
                };
                write_response_rerank(&writer, &resp, write_timeout).await?;
                continue;
            }
            Some(Err(SubmitError::Closed)) => {
                let resp = RerankResponse::Error {
                    id: resolved.id.clone(),
                    code: RerankErrorCode::BackendUnavailable,
                    message: "admission closed".into(),
                };
                write_response_rerank(&writer, &resp, write_timeout).await?;
                return Ok(());
            }
        };

        let dispatch = match router.dispatch_rerank() {
            Ok(d) => d,
            Err(RouterError::NoBackends) | Err(RouterError::NoneAvailable) => {
                let resp = RerankResponse::Error {
                    id: resolved.id.clone(),
                    code: RerankErrorCode::BackendUnavailable,
                    message: "no rerank-capable backend available".into(),
                };
                write_response_rerank(&writer, &resp, write_timeout).await?;
                continue;
            }
        };
        let backend_name = dispatch.name.clone();
        let backend = dispatch.backend;

        let req_id = resolved.id.clone();
        let n_documents = resolved.documents.len();

        match backend.rerank(resolved).await {
            Ok(out) => {
                let usage = out.usage;
                let n_results = out.results.len();
                let frame = RerankResponse::Rerank {
                    id: req_id.clone(),
                    results: out.results,
                    model: out.model,
                    usage,
                    backend: backend_name.clone(),
                };
                write_response_rerank(&writer, &frame, write_timeout).await?;
                router.record_success(&backend_name);
                info!(
                    target: "inferd_daemon::activity",
                    req_id = %req_id,
                    backend = %backend_name,
                    wire_version = "rerank",
                    n_documents = n_documents,
                    n_results = n_results,
                    input_tokens = usage.input_tokens,
                    "rerank_request_done"
                );
            }
            Err(e) => {
                let (code, message, is_backend_failure) = match e {
                    RerankError::InvalidRequest(m) => (RerankErrorCode::InvalidRequest, m, false),
                    RerankError::NotReady => (
                        RerankErrorCode::BackendUnavailable,
                        "backend not ready".into(),
                        true,
                    ),
                    RerankError::Unavailable(m) => (RerankErrorCode::BackendUnavailable, m, true),
                    RerankError::Unsupported => (
                        RerankErrorCode::RerankUnsupported,
                        "rerank not supported by this backend".into(),
                        false,
                    ),
                    RerankError::Internal(m) => (RerankErrorCode::Internal, m, true),
                };
                if is_backend_failure {
                    router.record_failure(&backend_name);
                }
                let frame = RerankResponse::Error {
                    id: req_id,
                    code,
                    message,
                };
                write_response_rerank(&writer, &frame, write_timeout).await?;
            }
        }
    }
}

fn error_code_for(e: &ProtoError) -> RerankErrorCode {
    match e {
        ProtoError::FrameTooLarge => RerankErrorCode::FrameTooLarge,
        ProtoError::Decode(_) | ProtoError::InvalidRequest(_) | ProtoError::MalformedFrame(_) => {
            RerankErrorCode::InvalidRequest
        }
        ProtoError::Io(_) => RerankErrorCode::Internal,
    }
}

async fn read_request_rerank<R>(reader: &mut R) -> Result<Option<RerankRequest>, ProtoError>
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
            return inferd_proto::read_frame::<&[u8], RerankRequest>(&mut &line[..]);
        }
        if let Some(idx) = buf.iter().position(|&b| b == b'\n') {
            if line.len() + idx > limit {
                return Err(ProtoError::FrameTooLarge);
            }
            line.extend_from_slice(&buf[..=idx]);
            reader.consume(idx + 1);
            return inferd_proto::read_frame::<&[u8], RerankRequest>(&mut &line[..]);
        }
        if line.len() + buf.len() > limit {
            return Err(ProtoError::FrameTooLarge);
        }
        line.extend_from_slice(buf);
        let n = buf.len();
        reader.consume(n);
    }
}

/// Write one NDJSON response frame, bounded by `timeout`
/// (THREAT_MODEL F-17). Same rationale as
/// `lifecycle_embed::write_response_embed`: the write happens downstream
/// of the admission gate that generation shares, so an unbounded write
/// here wedges generation slots too.
async fn write_response_rerank<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &RerankResponse,
    timeout: Option<std::time::Duration>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise rerank response: {e}")))?;
    let write = async {
        let mut guard = writer.lock().await;
        guard.write_all(&buf).await?;
        guard.flush().await?;
        Ok(())
    };
    match timeout {
        None => write.await,
        Some(d) => tokio::time::timeout(d, write).await.unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("peer did not accept a response frame within {d:?}"),
            ))
        }),
    }
}

/// Serve a rerank Unix domain socket listener.
#[cfg(unix)]
pub async fn serve_uds_rerank(
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!("rerank uds listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("rerank uds shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let peer = crate::peercred::unix::from_stream(&stream)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "rerank SO_PEERCRED failed; recording empty unix identity");
                        crate::peercred::PeerIdentity {
                            uid: None, gid: None, pid: None,
                            sid: None,
                            transport: "unix",
                        }
                    });
                let r = Arc::clone(&router);
                let ctx = ctx.clone();
                debug!(?peer, "rerank uds accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_rerank_connection(stream, r, peer, ctx).await {
                        warn!(error = ?e, "rerank connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve a rerank Windows named pipe listener.
#[cfg(windows)]
pub async fn serve_named_pipe_rerank(
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    use crate::endpoint::bind_named_pipe;

    info!(path = %path, "rerank named pipe listener accepting");
    let mut server = first_instance;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("rerank named pipe shutdown signalled");
                return Ok(());
            }
            connect_result = server.connect() => {
                connect_result?;
                let connected = server;
                server = bind_named_pipe(path, false)?;

                let peer = crate::peercred::windows::from_stream(&connected)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "rerank GetNamedPipeClientProcessId failed; empty pipe identity");
                        crate::peercred::PeerIdentity {
                            uid: None, gid: None, pid: None,
                            sid: None,
                            transport: "pipe",
                        }
                    });
                let r = Arc::clone(&router);
                let ctx = ctx.clone();
                debug!(?peer, "rerank named pipe accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_rerank_connection(connected, r, peer, ctx).await {
                        warn!(error = ?e, "rerank connection terminated with error");
                    }
                });
            }
        }
    }
}
