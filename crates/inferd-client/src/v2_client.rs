//! v2 inference-socket client. NDJSON over UDS / pipe / loopback TCP.
//!
//! Spec: ADR 0015. v2 lives on a *separate* socket from v1 — clients
//! pick a transport variant per platform with `dial_uds`,
//! `dial_pipe`, or `dial_tcp`, mirroring `Client`'s shape but using
//! the v2 wire types (`RequestV2` / `ResponseV2`). The framing
//! contract is identical: 64 MiB cap, `\n`-delimited NDJSON, terminal
//! `done` / `error` ends the stream.

use crate::client::ClientError;
use inferd_proto::v2::{RequestV2, ResponseV2};
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_stream::Stream;

/// Stream of `ResponseV2` frames yielded by `ClientV2::generate`.
pub type FrameStreamV2 = Pin<Box<dyn Stream<Item = Result<ResponseV2, ClientError>> + Send>>;

/// v2 inference-socket client.
///
/// Construct via `dial_tcp` / `dial_uds` (Unix) / `dial_pipe`
/// (Windows). Wrap with [`crate::dial_and_wait_ready`] to retry
/// connect during daemon bring-up — the retry helper is generic over
/// the client type so the same wait logic serves v1 and v2.
pub struct ClientV2 {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for ClientV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientV2").finish_non_exhaustive()
    }
}

struct Inner {
    write: Box<dyn AsyncWrite + Send + Unpin>,
    read: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

impl ClientV2 {
    /// Open a TCP connection to `addr` (e.g. `"127.0.0.1:47322"`).
    /// v2 TCP transport is opt-in (config-flagged off by default);
    /// see ADR 0015 §Endpoints for the configured port convention.
    pub async fn dial_tcp(addr: &str) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(addr).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Unix domain socket connection (Unix only). Default v2
    /// path: `${XDG_RUNTIME_DIR}/inferd/infer.v2.sock` on Linux,
    /// `${TMPDIR}/inferd/infer.v2.sock` on macOS.
    #[cfg(unix)]
    pub async fn dial_uds(path: &Path) -> Result<Self, ClientError> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Windows named pipe connection (Windows only). Default
    /// v2 path: `\\.\pipe\inferd-infer-v2`.
    #[cfg(windows)]
    pub async fn dial_pipe(path: &str) -> Result<Self, ClientError> {
        use tokio::net::windows::named_pipe::ClientOptions;
        let pipe = ClientOptions::new().open(path)?;
        let (read, write) = tokio::io::split(pipe);
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    fn wrap(
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                write,
                read: BufReader::with_capacity(64 * 1024, read),
            })),
        }
    }

    /// Test-only constructor: build a `ClientV2` from arbitrary
    /// `AsyncRead`/`AsyncWrite` halves. Lets sibling-module tests
    /// stub the transport with `tokio::io::duplex`. Not part of the
    /// public API.
    #[doc(hidden)]
    pub fn wrap_for_test(
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> Self {
        Self::wrap(read, write)
    }

    /// Send a `RequestV2` and return a stream of `ResponseV2` frames.
    /// The stream completes after a terminal `done` or `error` frame
    /// (or yields `Err(ClientError::UnexpectedEof)` if the daemon
    /// closes the connection mid-stream).
    pub async fn generate(&mut self, req: RequestV2) -> Result<FrameStreamV2, ClientError> {
        let mut buf = Vec::with_capacity(512);
        serde_json::to_writer(&mut buf, &req)?;
        buf.push(b'\n');

        {
            let mut g = self.inner.lock().await;
            g.write.write_all(&buf).await?;
            g.write.flush().await?;
        }

        let inner = Arc::clone(&self.inner);
        let stream = async_stream::stream! {
            loop {
                let mut g = inner.lock().await;
                let mut line = Vec::with_capacity(512);
                let n = match g.read.read_until(b'\n', &mut line).await {
                    Ok(n) => n,
                    Err(e) => { yield Err(ClientError::Io(e)); return; }
                };
                if n == 0 {
                    yield Err(ClientError::UnexpectedEof);
                    return;
                }
                drop(g);

                match serde_json::from_slice::<ResponseV2>(&line) {
                    Ok(resp) => {
                        let terminal = resp.is_terminal();
                        yield Ok(resp);
                        if terminal {
                            return;
                        }
                    }
                    Err(e) => {
                        yield Err(ClientError::Decode(e));
                        return;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

/// Default v2 admin / inference endpoint paths, mirroring the
/// daemon's `endpoint::default_v2_addr`. Returned as `PathBuf` on
/// Unix and as a pipe-path string on Windows; callers pick by `cfg`.
///
/// Linux fallback chain (same as v1's admin chain):
/// 1. `${XDG_RUNTIME_DIR}/inferd/infer.v2.sock`
/// 2. `${HOME}/.inferd/run/infer.v2.sock`
/// 3. `/tmp/inferd/infer.v2.sock`
pub fn default_v2_addr() -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            let mut p = std::path::PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                p.push("inferd");
                p.push("infer.v2.sock");
                return p;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = std::path::PathBuf::from(home);
            if !p.as_os_str().is_empty() {
                p.push(".inferd");
                p.push("run");
                p.push("infer.v2.sock");
                return p;
            }
        }
        std::path::PathBuf::from("/tmp/inferd/infer.v2.sock")
    }
    #[cfg(target_os = "macos")]
    {
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push("infer.v2.sock");
        p
    }
    #[cfg(windows)]
    {
        std::path::PathBuf::from(r"\\.\pipe\inferd-infer-v2")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        std::path::PathBuf::from("/tmp/inferd/infer.v2.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_proto::v2::{
        ContentBlock, ErrorCodeV2, MessageV2, ResponseBlock, RoleV2, StopReasonV2, UsageV2,
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    fn sample_request() -> RequestV2 {
        RequestV2 {
            id: "v2-test".into(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn generate_streams_frame_then_done() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = ClientV2::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();

            let frame = serde_json::to_vec(&ResponseV2::Frame {
                id: "v2-test".into(),
                block: ResponseBlock::Text { delta: "hi".into() },
            })
            .unwrap();
            tx.write_all(&frame).await.unwrap();
            tx.write_all(b"\n").await.unwrap();

            let done = serde_json::to_vec(&ResponseV2::Done {
                id: "v2-test".into(),
                usage: UsageV2 {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                stop_reason: StopReasonV2::EndTurn,
                backend: "mock".into(),
            })
            .unwrap();
            tx.write_all(&done).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
        });

        let stream = client.generate(sample_request()).await.unwrap();
        use tokio_stream::StreamExt;
        let frames: Vec<_> = stream.collect().await;
        server.await.unwrap();

        assert_eq!(frames.len(), 2);
        match frames[0].as_ref().unwrap() {
            ResponseV2::Frame {
                block: ResponseBlock::Text { delta },
                ..
            } => assert_eq!(delta, "hi"),
            other => panic!("frame[0]: {other:?}"),
        }
        match frames[1].as_ref().unwrap() {
            ResponseV2::Done {
                backend,
                stop_reason,
                ..
            } => {
                assert_eq!(backend, "mock");
                assert_eq!(*stop_reason, StopReasonV2::EndTurn);
            }
            other => panic!("frame[1]: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unexpected_eof_yields_clienterror() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = ClientV2::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, _tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();
            // server_side drops here -> EOF on client.
        });

        let mut stream = client.generate(sample_request()).await.unwrap();
        use tokio_stream::StreamExt;
        let first = stream.next().await.unwrap();
        server.await.unwrap();
        match first {
            Err(ClientError::UnexpectedEof) => {}
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[test]
    fn error_v2_round_trips() {
        let frame = ResponseV2::Error {
            id: "x".into(),
            code: ErrorCodeV2::AttachmentUnsupported,
            message: "no audio".into(),
        };
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains(r#""code":"attachment_unsupported""#));
    }
}
