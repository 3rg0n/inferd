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

use crate::endpoint::Connection;
use crate::lifecycle::{
    CONNECTION_READ_BUFFER_BYTES, proto_error_code, read_ndjson_request, write_bounded,
};
use crate::peercred::PeerIdentity;
use crate::queue::SubmitError;
use crate::router::{Router, RouterError};
use inferd_engine::EmbedError;
use inferd_proto::ProtoError;
use inferd_proto::embed::{EmbedErrorCode, EmbedRequest, EmbedResponse};
use inferd_proto::write_frame;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, BufReader};
use tokio::sync::Mutex;
use tracing::info;

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
    let mut reader = BufReader::with_capacity(CONNECTION_READ_BUFFER_BYTES, read_half);
    let writer = Arc::new(Mutex::new(write_half));
    // THREAT_MODEL F-17: bounded so a peer that stops reading can't hold
    // the shared admission permit indefinitely.
    let write_timeout = ctx.write_timeout;

    loop {
        let request: EmbedRequest = match read_ndjson_request(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                let resp = EmbedResponse::Error {
                    id: String::new(),
                    code: proto_error_code(&e),
                    message: e.to_string(),
                };
                write_response_embed(&writer, &resp, write_timeout).await?;
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
                write_response_embed(&writer, &resp, write_timeout).await?;
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
                write_response_embed(&writer, &resp, write_timeout).await?;
                continue;
            }
            Some(Err(SubmitError::Closed)) => {
                let resp = EmbedResponse::Error {
                    id: resolved.id.clone(),
                    code: EmbedErrorCode::BackendUnavailable,
                    message: "admission closed".into(),
                };
                write_response_embed(&writer, &resp, write_timeout).await?;
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
                write_response_embed(&writer, &resp, write_timeout).await?;
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
                write_response_embed(&writer, &frame, write_timeout).await?;
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
                write_response_embed(&writer, &frame, write_timeout).await?;
            }
        }
    }
}

/// Write one NDJSON embed response frame, bounded by the per-write
/// timeout (THREAT_MODEL F-17; see [`write_bounded`]).
async fn write_response_embed<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &EmbedResponse,
    timeout: Option<std::time::Duration>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise embed response: {e}")))?;
    write_bounded(writer, &buf, timeout).await
}

/// Serve an embed Unix domain socket listener.
#[cfg(unix)]
pub async fn serve_uds_embed(
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    ctx: AcceptContext,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    crate::lifecycle::serve_uds(
        "embed",
        listener,
        router,
        ctx,
        shutdown,
        handle_embed_connection,
    )
    .await
}

/// Serve an embed Windows named pipe listener.
#[cfg(windows)]
pub async fn serve_named_pipe_embed(
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    router: Arc<Router>,
    ctx: AcceptContext,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    crate::lifecycle::serve_named_pipe(
        "embed",
        path,
        first_instance,
        router,
        ctx,
        shutdown,
        handle_embed_connection,
    )
    .await
}
