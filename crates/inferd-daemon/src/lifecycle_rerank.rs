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
use crate::lifecycle::{
    CONNECTION_READ_BUFFER_BYTES, proto_error_code, read_ndjson_request, write_bounded,
};
use crate::peercred::PeerIdentity;
use crate::queue::SubmitError;
use crate::router::{Router, RouterError};
use inferd_engine::RerankError;
use inferd_proto::ProtoError;
use inferd_proto::rerank::{RerankErrorCode, RerankRequest, RerankResponse};
use inferd_proto::write_frame;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, BufReader};
use tokio::sync::Mutex;
use tracing::info;

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
    let mut reader = BufReader::with_capacity(CONNECTION_READ_BUFFER_BYTES, read_half);
    let writer = Arc::new(Mutex::new(write_half));
    // THREAT_MODEL F-17: bounded so a peer that stops reading can't hold
    // the shared admission permit indefinitely.
    let write_timeout = ctx.write_timeout;

    loop {
        let request: RerankRequest = match read_ndjson_request(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                let resp = RerankResponse::Error {
                    id: String::new(),
                    code: proto_error_code(&e),
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

/// Write one NDJSON rerank response frame, bounded by the per-write
/// timeout (THREAT_MODEL F-17; see [`write_bounded`]).
async fn write_response_rerank<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &RerankResponse,
    timeout: Option<std::time::Duration>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise rerank response: {e}")))?;
    write_bounded(writer, &buf, timeout).await
}

/// Serve a rerank Unix domain socket listener.
#[cfg(unix)]
pub async fn serve_uds_rerank(
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    ctx: AcceptContext,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    crate::lifecycle::serve_uds(
        "rerank",
        listener,
        router,
        ctx,
        shutdown,
        handle_rerank_connection,
    )
    .await
}

/// Serve a rerank Windows named pipe listener.
#[cfg(windows)]
pub async fn serve_named_pipe_rerank(
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    router: Arc<Router>,
    ctx: AcceptContext,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    crate::lifecycle::serve_named_pipe(
        "rerank",
        path,
        first_instance,
        router,
        ctx,
        shutdown,
        handle_rerank_connection,
    )
    .await
}
