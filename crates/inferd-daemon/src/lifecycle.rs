//! Daemon lifecycle primitives shared across every wire surface:
//! readiness gating ([`wait_for_ready`]) and the per-accept
//! [`AcceptContext`].
//!
//! As of v0.4 (ADR 0021) the generation connection handler and its
//! listeners live in [`crate::lifecycle_v2`] — the original text-only
//! v1 NDJSON path was removed when v1 was folded into v2. This module
//! keeps only the transport-agnostic pieces the v2 and embed
//! lifecycles both build on:
//! - `lock` — single-instance lock at startup (THREAT_MODEL F-2).
//! - `router` — backend selection.
//! - `endpoint` — listener bound only after `router.all_ready()`
//!   (THREAT_MODEL F-13).
//! - `queue` — admission gate (`SubmitError::QueueFull` → wire
//!   `code: queue_full`).
//!
//! It also holds the plumbing every surface's lifecycle shares rather
//! than copies: the bounded response write ([`write_bounded`]), the NDJSON
//! request reader embed and rerank both frame with
//! ([`read_ndjson_request`]), the `ProtoError` → wire-code mapping
//! ([`proto_error_code`]), and the per-transport accept loops
//! ([`serve_uds`] / [`serve_named_pipe`]). Each surface keeps only what is
//! actually different about it — its request handler.

use crate::peercred::PeerIdentity;
use crate::queue::Admission;
use crate::router::Router;
use inferd_proto::ProtoError;
use inferd_proto::embed::EmbedErrorCode;
use inferd_proto::rerank::RerankErrorCode;
use inferd_proto::v2::ErrorCodeV2;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Read-side buffer every surface wraps its connection in.
pub(crate) const CONNECTION_READ_BUFFER_BYTES: usize = 64 * 1024;

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

/// Default ceiling on a single response write (THREAT_MODEL F-17).
///
/// A response write blocks once the peer's receive buffer fills and the
/// peer stops reading. Because that write happens while the request
/// holds its admission permit, an unbounded wait converts one
/// stopped-reading client into a permanently occupied generation slot.
/// This bounds the wait.
///
/// 60s is chosen to be far longer than any legitimate write can take —
/// a response frame is at most a few hundred KiB and the peer is a local
/// process, so the only way to exceed it is a peer that has stopped
/// reading entirely — while still short enough that a wedged slot
/// recovers without operator intervention.
pub const DEFAULT_WRITE_TIMEOUT_SECS: u64 = 60;

/// Per-accept context that the lifecycle hands to every spawned
/// connection task.
///
/// Today it carries the shared admission gate (queue_full enforcement)
/// and the per-write timeout (F-17). New per-connection policy (rate
/// limits, per-caller quotas) extends this struct rather than each
/// `serve_*` signature. (Peer identity is kernel-attested per transport
/// — UDS/pipe, F-7 — so there is no in-band API-key field; inbound TCP
/// was removed in ADR 0022.)
#[derive(Clone)]
pub struct AcceptContext {
    /// Shared admission gate. `None` for tests / dev paths that
    /// don't care about queue depth — those treat every request
    /// as admitted. Production lifecycle always passes `Some`.
    pub admission: Option<Admission>,
    /// Ceiling on one response write, after which the connection is
    /// dropped and the admission permit released (THREAT_MODEL F-17).
    /// Defaults to [`DEFAULT_WRITE_TIMEOUT_SECS`]; tests shorten it to
    /// make the wedge reproducible in bounded time. `None` disables the
    /// bound — never set by the daemon binary, and only correct for a
    /// test that owns both ends of the socket.
    pub write_timeout: Option<Duration>,
}

impl Default for AcceptContext {
    fn default() -> Self {
        Self {
            admission: None,
            write_timeout: Some(Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS)),
        }
    }
}

impl std::fmt::Debug for AcceptContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptContext")
            .field(
                "admission_capacity",
                &self.admission.as_ref().map(|a| a.capacity()),
            )
            .field("write_timeout", &self.write_timeout)
            .finish()
    }
}

/// A surface's wire error-code enum, as far as transport-level
/// [`ProtoError`]s are concerned.
///
/// Every surface answers the same three ways to a frame it could not
/// read; only the enum the answer is spelled in differs. Implemented here
/// rather than per-lifecycle so a new `ProtoError` variant is classified
/// once — the exhaustive match in [`proto_error_code`] is the single place
/// the compiler sends you.
pub(crate) trait ProtoErrorCode {
    /// The frame exceeded the 64 MiB cap (THREAT_MODEL F-5).
    const FRAME_TOO_LARGE: Self;
    /// The frame was readable but not a valid request.
    const INVALID_REQUEST: Self;
    /// Anything else.
    const INTERNAL: Self;
}

impl ProtoErrorCode for ErrorCodeV2 {
    const FRAME_TOO_LARGE: Self = ErrorCodeV2::FrameTooLarge;
    const INVALID_REQUEST: Self = ErrorCodeV2::InvalidRequest;
    const INTERNAL: Self = ErrorCodeV2::Internal;
}

impl ProtoErrorCode for EmbedErrorCode {
    const FRAME_TOO_LARGE: Self = EmbedErrorCode::FrameTooLarge;
    const INVALID_REQUEST: Self = EmbedErrorCode::InvalidRequest;
    const INTERNAL: Self = EmbedErrorCode::Internal;
}

impl ProtoErrorCode for RerankErrorCode {
    const FRAME_TOO_LARGE: Self = RerankErrorCode::FrameTooLarge;
    const INVALID_REQUEST: Self = RerankErrorCode::InvalidRequest;
    const INTERNAL: Self = RerankErrorCode::Internal;
}

/// Map a transport-level read error to the surface's wire code.
pub(crate) fn proto_error_code<C: ProtoErrorCode>(e: &ProtoError) -> C {
    match e {
        ProtoError::FrameTooLarge => C::FRAME_TOO_LARGE,
        ProtoError::Decode(_) | ProtoError::InvalidRequest(_) | ProtoError::MalformedFrame(_) => {
            C::INVALID_REQUEST
        }
        ProtoError::Io(_) => C::INTERNAL,
    }
}

/// Read one NDJSON request frame (the embed and rerank framing, ADR 0017).
///
/// `Ok(None)` on a clean EOF between frames. The 64 MiB cap
/// (THREAT_MODEL F-5) is enforced on the bytes accumulated so far, before
/// the newline is found, so an unterminated line cannot grow the buffer
/// past the cap.
pub(crate) async fn read_ndjson_request<R, T>(reader: &mut R) -> Result<Option<T>, ProtoError>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    let mut line = Vec::with_capacity(512);
    let limit = inferd_proto::MAX_FRAME_BYTES;
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return inferd_proto::read_frame::<&[u8], T>(&mut &line[..]);
        }
        if let Some(idx) = buf.iter().position(|&b| b == b'\n') {
            if line.len() + idx > limit {
                return Err(ProtoError::FrameTooLarge);
            }
            line.extend_from_slice(&buf[..=idx]);
            reader.consume(idx + 1);
            return inferd_proto::read_frame::<&[u8], T>(&mut &line[..]);
        }
        if line.len() + buf.len() > limit {
            return Err(ProtoError::FrameTooLarge);
        }
        line.extend_from_slice(buf);
        let n = buf.len();
        reader.consume(n);
    }
}

/// Write one already-encoded response frame, bounded by `timeout`
/// (THREAT_MODEL F-17).
///
/// Every surface's response write happens while the request holds its
/// admission permit — the gate generation, embed and rerank share — so an
/// unbounded write to a peer that stopped reading wedges a generation
/// slot whichever surface the peer is on.
///
/// The bound covers acquiring the writer mutex as well as the write: a
/// peer stalled inside `write_all` holds that mutex.
pub(crate) async fn write_bounded<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    frame: &[u8],
    timeout: Option<Duration>,
) -> io::Result<()> {
    let write = async {
        let mut guard = writer.lock().await;
        guard.write_all(frame).await?;
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

/// Peer identity recorded when the kernel lookup fails. The connection is
/// still served — transport access is already gated by the socket's mode
/// / the pipe's DACL — but the activity log shows the identity as empty
/// rather than inventing one.
fn empty_identity(transport: &'static str) -> PeerIdentity {
    PeerIdentity {
        uid: None,
        gid: None,
        pid: None,
        sid: None,
        transport,
    }
}

/// Serve one surface's Unix domain socket listener: accept, attest the
/// peer (SO_PEERCRED), and spawn `handler` per connection until
/// `shutdown` fires.
#[cfg(unix)]
pub(crate) async fn serve_uds<H, F>(
    surface: &'static str,
    listener: tokio::net::UnixListener,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
    handler: H,
) -> io::Result<()>
where
    H: Fn(tokio::net::UnixStream, Arc<Router>, PeerIdentity, AcceptContext) -> F,
    F: Future<Output = io::Result<()>> + Send + 'static,
{
    info!("{surface} uds listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("{surface} uds shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let peer = crate::peercred::unix::from_stream(&stream).unwrap_or_else(|e| {
                    warn!(error = %e, "{surface} SO_PEERCRED failed; recording empty unix identity");
                    empty_identity("unix")
                });
                debug!(?peer, "{surface} uds accept");
                let conn = handler(stream, Arc::clone(&router), peer, ctx.clone());
                tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        warn!(error = ?e, "{surface} connection terminated with error");
                    }
                });
            }
        }
    }
}

/// Serve one surface's Windows named pipe listener: accept, re-arm the
/// next pipe instance, attest the peer, and spawn `handler` per
/// connection until `shutdown` fires.
#[cfg(windows)]
pub(crate) async fn serve_named_pipe<H, F>(
    surface: &'static str,
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    router: Arc<Router>,
    ctx: AcceptContext,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
    handler: H,
) -> io::Result<()>
where
    H: Fn(
        tokio::net::windows::named_pipe::NamedPipeServer,
        Arc<Router>,
        PeerIdentity,
        AcceptContext,
    ) -> F,
    F: Future<Output = io::Result<()>> + Send + 'static,
{
    use crate::endpoint::bind_named_pipe;

    info!(path = %path, "{surface} named pipe listener accepting");
    let mut server = first_instance;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("{surface} named pipe shutdown signalled");
                return Ok(());
            }
            connect_result = server.connect() => {
                connect_result?;
                let connected = server;
                server = bind_named_pipe(path, false)?;

                let peer = crate::peercred::windows::from_stream(&connected).unwrap_or_else(|e| {
                    warn!(error = %e, "{surface} GetNamedPipeClientProcessId failed; empty pipe identity");
                    empty_identity("pipe")
                });
                debug!(?peer, "{surface} named pipe accept");
                let conn = handler(connected, Arc::clone(&router), peer, ctx.clone());
                tokio::spawn(async move {
                    if let Err(e) = conn.await {
                        warn!(error = ?e, "{surface} connection terminated with error");
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
    use std::sync::Arc;

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
