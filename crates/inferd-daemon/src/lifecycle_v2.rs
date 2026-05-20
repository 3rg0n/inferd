//! v2 connection lifecycle — Phase 2A wired.
//!
//! Per ADR 0015, v2 lives on a *separate* socket from v1. This module
//! mirrors `lifecycle.rs` but for the v2 wire types
//! (`inferd_proto::v2::RequestV2` / `ResponseV2`).
//!
//! Per request:
//!   1. Read one NDJSON frame, parse as `RequestV2`.
//!   2. `RequestV2::resolve()` — structural validation.
//!   3. Admission gate (same Admission shared with v1; one slot
//!      is one slot regardless of wire version).
//!   4. Dispatch through the router; check the chosen backend's
//!      `capabilities().v2` flag — backends that don't support v2
//!      yield `Error{Internal, "v2 not supported by this backend"}`.
//!   5. `backend.generate_v2(resolved)` — pre-stream errors map to
//!      v2 error codes; mid-stream backend failure (no Done) maps to
//!      `BackendUnavailable`.
//!   6. Stream `TokenEventV2`s, translating each to the
//!      corresponding `ResponseV2::Frame` / `Done`.

use crate::auth::{AuthFrame, key_matches};
use crate::endpoint::Connection;
use crate::peercred::PeerIdentity;
use crate::queue::SubmitError;
use crate::router::{Router, RouterError};
use inferd_engine::{GenerateError, TokenEventV2};
use inferd_proto::ProtoError;
use inferd_proto::v2::{ErrorCodeV2, RequestV2, ResponseBlock, ResponseV2};
use inferd_proto::write_frame;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

/// Per-accept context for v2 connections. v2 reuses v1's
/// `AcceptContext` shape — same TCP API key, same admission
/// gate. The admission gate, in particular, sees v1 and v2
/// requests as equivalent: one slot is one slot.
pub use crate::lifecycle::AcceptContext;

/// Handle one accepted v2 client connection.
pub async fn handle_v2_connection<C: Connection + 'static>(
    mut conn: C,
    router: Arc<Router>,
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

        // Admission gate. v1 and v2 share one Admission instance; a
        // v2 in-flight request occupies the same slot a v1 one would.
        let _admit_permit = match ctx.admission.as_ref().map(|a| a.try_admit()) {
            None => None,
            Some(Ok(p)) => Some(p),
            Some(Err(SubmitError::QueueFull)) => {
                let resp = ResponseV2::Error {
                    id: resolved.id.clone(),
                    code: ErrorCodeV2::QueueFull,
                    message: "queue full".into(),
                };
                write_response_v2(&writer, &resp).await?;
                continue;
            }
            Some(Err(SubmitError::Closed)) => {
                let resp = ResponseV2::Error {
                    id: resolved.id.clone(),
                    code: ErrorCodeV2::BackendUnavailable,
                    message: "admission closed".into(),
                };
                write_response_v2(&writer, &resp).await?;
                return Ok(());
            }
        };

        // Dispatch through the router.
        let backend = match router.dispatch() {
            Ok(b) => b,
            Err(RouterError::NoBackends) | Err(RouterError::NoneAvailable) => {
                let resp = ResponseV2::Error {
                    id: resolved.id.clone(),
                    code: ErrorCodeV2::BackendUnavailable,
                    message: "no backend available".into(),
                };
                write_response_v2(&writer, &resp).await?;
                continue;
            }
        };

        // Backends that don't advertise v2 capability are not given a
        // generate_v2 call — the trait's default impl would also
        // refuse, but checking up front lets us emit a clearer error
        // and avoids paying for a method-dispatch round-trip just to
        // surface "not supported".
        if !backend.capabilities().v2 {
            let resp = ResponseV2::Error {
                id: resolved.id.clone(),
                code: ErrorCodeV2::Internal,
                message: format!(
                    "backend {:?} does not advertise v2 capability",
                    backend.name()
                ),
            };
            write_response_v2(&writer, &resp).await?;
            continue;
        }

        let backend_name = backend.name().to_string();
        let req_id = resolved.id.clone();
        let n_attachments = resolved.attachments.len();
        let n_tools = resolved.tools.len();

        let mut stream = match backend.generate_v2(resolved).await {
            Ok(s) => s,
            Err(e) => {
                let (code, message) = match e {
                    GenerateError::InvalidRequest(m) => (ErrorCodeV2::InvalidRequest, m),
                    GenerateError::NotReady => {
                        (ErrorCodeV2::BackendUnavailable, "backend not ready".into())
                    }
                    GenerateError::Unavailable(m) => (ErrorCodeV2::BackendUnavailable, m),
                    GenerateError::Internal(m) => (ErrorCodeV2::Internal, m),
                };
                let resp = ResponseV2::Error {
                    id: req_id,
                    code,
                    message,
                };
                write_response_v2(&writer, &resp).await?;
                continue;
            }
        };

        let mut terminal_emitted = false;
        while let Some(ev) = stream.next().await {
            match ev {
                TokenEventV2::Text(delta) => {
                    let frame = ResponseV2::Frame {
                        id: req_id.clone(),
                        block: ResponseBlock::Text { delta },
                    };
                    write_response_v2(&writer, &frame).await?;
                }
                TokenEventV2::Thinking(delta) => {
                    let frame = ResponseV2::Frame {
                        id: req_id.clone(),
                        block: ResponseBlock::Thinking { delta },
                    };
                    write_response_v2(&writer, &frame).await?;
                }
                TokenEventV2::ToolUse {
                    tool_call_id,
                    name,
                    input,
                } => {
                    let frame = ResponseV2::Frame {
                        id: req_id.clone(),
                        block: ResponseBlock::ToolUse {
                            tool_call_id,
                            name,
                            input,
                        },
                    };
                    write_response_v2(&writer, &frame).await?;
                }
                TokenEventV2::Done { stop_reason, usage } => {
                    let frame = ResponseV2::Done {
                        id: req_id.clone(),
                        usage,
                        stop_reason,
                        backend: backend_name.clone(),
                    };
                    write_response_v2(&writer, &frame).await?;
                    info!(
                        target: "inferd_daemon::activity",
                        req_id = %req_id,
                        backend = %backend_name,
                        wire_version = "v2",
                        stop_reason = ?stop_reason,
                        input_tokens = usage.input_tokens,
                        output_tokens = usage.output_tokens,
                        n_attachments = n_attachments,
                        n_tools = n_tools,
                        "v2_request_done"
                    );
                    terminal_emitted = true;
                    break;
                }
            }
        }

        if !terminal_emitted {
            warn!(
                target: "inferd_daemon::activity",
                req_id = %req_id,
                backend = %backend_name,
                wire_version = "v2",
                "v2_request_error_mid_stream"
            );
            let frame = ResponseV2::Error {
                id: req_id,
                code: ErrorCodeV2::BackendUnavailable,
                message: "backend ended stream without terminal frame".into(),
            };
            write_response_v2(&writer, &frame).await?;
        }
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
    router: Arc<Router>,
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
                let r = Arc::clone(&router);
                let ctx = ctx.clone();
                debug!(?peer_addr, "v2 tcp accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_v2_connection(stream, r, peer, ctx).await {
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
    router: Arc<Router>,
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
                let r = Arc::clone(&router);
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
                    if let Err(e) = handle_v2_connection(stream, r, peer, ctx).await {
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
    router: Arc<Router>,
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
                let r = Arc::clone(&router);
                let ctx = ctx.clone();
                debug!(?peer, "v2 named pipe accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_v2_connection(connected, r, peer, ctx).await {
                        warn!(error = ?e, "v2 connection terminated with error");
                    }
                });
            }
        }
    }
}
