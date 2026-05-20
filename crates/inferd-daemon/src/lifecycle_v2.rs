//! v2 connection lifecycle — Phase 1B stub.
//!
//! Per ADR 0015, v2 lives on a *separate* socket from v1. This module
//! mirrors `lifecycle.rs` but for the v2 wire types
//! (`inferd_proto::v2::RequestV2` / `ResponseV2`).
//!
//! Phase 1B scope: bind v2 listeners, accept connections, parse and
//! validate `RequestV2` frames, and respond with one of:
//!
//! - `ResponseV2::Error{InvalidRequest, ...}` for malformed JSON or
//!   any structural validation failure (e.g. dangling attachment id,
//!   duplicate tool name) — same behaviour as v1.
//! - `ResponseV2::Error{Internal, "v2 generation not implemented"}`
//!   for a successfully-validated request. The Backend trait does
//!   not yet expose a `generate_v2` method (Phase 2A); until it
//!   does, we tell callers we received their request shape but
//!   have no engine path to satisfy it.
//!
//! This lets middleware authors integrate against the v2 socket
//! today: connect, send a typed-content-block request, get a clean
//! protocol error — and update only their generation handler when
//! Phase 2A lands.

use crate::auth::{AuthFrame, key_matches};
use crate::endpoint::Connection;
use crate::peercred::PeerIdentity;
use inferd_proto::ProtoError;
use inferd_proto::v2::{ErrorCodeV2, RequestV2, ResponseV2};
use inferd_proto::write_frame;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Per-accept context for v2 connections. v2 reuses v1's
/// `AcceptContext` shape — same TCP API key, same admission
/// gate. The admission gate, in particular, sees v1 and v2
/// requests as equivalent: one slot is one slot.
pub use crate::lifecycle::AcceptContext;

/// Handle one accepted v2 client connection.
///
/// Per request:
/// 1. Read one NDJSON frame, parse as `RequestV2`.
/// 2. `RequestV2::resolve()` — structural validation.
/// 3. Until Phase 2A: emit `Error{Internal, "v2 generation not
///    implemented"}` and continue to the next request on the same
///    connection. Once Phase 2A lands, dispatch the resolved
///    request through the router's `generate_v2` path.
pub async fn handle_v2_connection<C: Connection + 'static>(
    mut conn: C,
    peer: PeerIdentity,
    ctx: AcceptContext,
) -> Result<(), io::Error> {
    let transport = conn.transport();
    info!(
        target: "inferd_daemon::activity",
        transport = transport,
        wire_version = "v2",
        peer = %peer,
        peer_uid = peer.uid,
        peer_pid = peer.pid,
        peer_sid = peer.sid.as_deref(),
        "v2_connection_accepted"
    );

    let (read_half, write_half) = tokio::io::split(&mut conn);
    let mut reader = BufReader::with_capacity(64 * 1024, read_half);
    let writer = Arc::new(Mutex::new(write_half));

    // F-8 first-frame auth on TCP, identical to v1.
    if transport == "tcp"
        && let Some(expected) = ctx.expected_api_key.as_deref()
    {
        match read_auth_frame(&mut reader).await {
            Some(frame) if key_matches(&frame.key, expected) => {
                debug!(transport, "v2 tcp auth ok");
            }
            _ => {
                warn!(
                    target: "inferd_daemon::activity",
                    peer = %peer,
                    wire_version = "v2",
                    "v2_tcp_auth_rejected"
                );
                return Ok(());
            }
        }
    }

    loop {
        let request: RequestV2 = match read_request_v2(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                let resp = ResponseV2::Error {
                    id: String::new(),
                    code: error_code_for(&e),
                    message: e.to_string(),
                };
                write_response_v2(&writer, &resp).await?;
                return Ok(());
            }
        };

        let id = request.id.clone();
        let resolved = match request.resolve() {
            Ok(r) => r,
            Err(e) => {
                let resp = ResponseV2::Error {
                    id,
                    code: ErrorCodeV2::InvalidRequest,
                    message: e.to_string(),
                };
                write_response_v2(&writer, &resp).await?;
                continue;
            }
        };

        // Phase 2A will replace this with router.dispatch_v2() +
        // backend.generate_v2(resolved). Until then: clean error
        // back to the caller.
        info!(
            target: "inferd_daemon::activity",
            req_id = %resolved.id,
            n_messages = resolved.messages.len(),
            n_attachments = resolved.attachments.len(),
            n_tools = resolved.tools.len(),
            "v2_request_received_not_implemented"
        );
        let resp = ResponseV2::Error {
            id: resolved.id,
            code: ErrorCodeV2::Internal,
            message: "v2 generation not implemented (Phase 2A pending)".into(),
        };
        write_response_v2(&writer, &resp).await?;
    }
}

fn error_code_for(e: &ProtoError) -> ErrorCodeV2 {
    match e {
        ProtoError::FrameTooLarge => ErrorCodeV2::FrameTooLarge,
        ProtoError::Decode(_) | ProtoError::InvalidRequest(_) => ErrorCodeV2::InvalidRequest,
        ProtoError::Io(_) => ErrorCodeV2::Internal,
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

async fn read_request_v2<R>(reader: &mut R) -> Result<Option<RequestV2>, ProtoError>
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
            return inferd_proto::read_frame::<&[u8], RequestV2>(&mut &line[..]);
        }
        if let Some(idx) = buf.iter().position(|&b| b == b'\n') {
            if line.len() + idx > limit {
                return Err(ProtoError::FrameTooLarge);
            }
            line.extend_from_slice(&buf[..=idx]);
            reader.consume(idx + 1);
            return inferd_proto::read_frame::<&[u8], RequestV2>(&mut &line[..]);
        }
        if line.len() + buf.len() > limit {
            return Err(ProtoError::FrameTooLarge);
        }
        line.extend_from_slice(buf);
        let n = buf.len();
        reader.consume(n);
    }
}

async fn write_response_v2<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &ResponseV2,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise v2 response: {e}")))?;
    let mut guard = writer.lock().await;
    guard.write_all(&buf).await?;
    guard.flush().await?;
    Ok(())
}

/// Serve a v2 TCP listener.
pub async fn serve_tcp_v2(
    listener: tokio::net::TcpListener,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!(addr = ?listener.local_addr()?, "v2 tcp listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("v2 tcp shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, peer_addr) = accept?;
                let peer = PeerIdentity::from_tcp(peer_addr);
                let ctx = ctx.clone();
                debug!(?peer_addr, "v2 tcp accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_v2_connection(stream, peer, ctx).await {
                        warn!(error = ?e, "v2 connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve a v2 Unix domain socket listener.
#[cfg(unix)]
pub async fn serve_uds_v2(
    listener: tokio::net::UnixListener,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!("v2 uds listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("v2 uds shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let peer = crate::peercred::unix::from_stream(&stream)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "v2 SO_PEERCRED failed; recording empty unix identity");
                        crate::peercred::PeerIdentity {
                            uid: None, gid: None, pid: None,
                            sid: None, remote_addr: None,
                            transport: "unix",
                        }
                    });
                let ctx = ctx.clone();
                debug!(?peer, "v2 uds accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_v2_connection(stream, peer, ctx).await {
                        warn!(error = ?e, "v2 connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve a v2 Windows named pipe listener.
#[cfg(windows)]
pub async fn serve_named_pipe_v2(
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    use crate::endpoint::bind_named_pipe;

    info!(path = %path, "v2 named pipe listener accepting");
    let mut server = first_instance;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("v2 named pipe shutdown signalled");
                return Ok(());
            }
            connect_result = server.connect() => {
                connect_result?;
                let connected = server;
                server = bind_named_pipe(path, false)?;

                let peer = crate::peercred::windows::from_stream(&connected)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "v2 GetNamedPipeClientProcessId failed; empty pipe identity");
                        crate::peercred::PeerIdentity {
                            uid: None, gid: None, pid: None,
                            sid: None, remote_addr: None,
                            transport: "pipe",
                        }
                    });
                let ctx = ctx.clone();
                debug!(?peer, "v2 named pipe accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_v2_connection(connected, peer, ctx).await {
                        warn!(error = ?e, "v2 connection terminated with error");
                    }
                });
            }
        }
    }
}
