//! Rerank-socket client. NDJSON over UDS / named pipe.
//!
//! Spec: ADR 0027. The rerank socket is the *fourth* inferd surface
//! (generation, embed, rerank, plus admin), each on its own path.
//! Construct a `RerankClient` with `dial_uds` (Unix) or `dial_pipe`
//! (Windows), then call `rerank` per request. The connection is
//! long-lived: send a request, receive one terminal frame, send the
//! next. Nothing streams — a rerank result is a complete ordering, and
//! a partial ordering isn't useful.

use crate::client::ClientError;
use inferd_proto::rerank::{RerankRequest, RerankResponse};
#[cfg(unix)]
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Rerank-socket client.
///
/// Construct via `dial_uds` (Unix) or `dial_pipe` (Windows). Wrap with
/// [`crate::dial_and_wait_ready`] to retry connect during daemon
/// bring-up — the retry helper is generic over the client type.
pub struct RerankClient {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for RerankClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RerankClient").finish_non_exhaustive()
    }
}

struct Inner {
    write: Box<dyn AsyncWrite + Send + Unpin>,
    read: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

impl RerankClient {
    /// Open a Unix domain socket connection (Unix only). Default rerank
    /// path: `${XDG_RUNTIME_DIR}/inferd/infer.rerank.sock` on Linux,
    /// `${TMPDIR}/inferd/infer.rerank.sock` on macOS.
    #[cfg(unix)]
    pub async fn dial_uds(path: &Path) -> Result<Self, ClientError> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Windows named pipe connection (Windows only). Default
    /// rerank path: `\\.\pipe\inferd-infer-rerank`.
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

    /// Test-only constructor: build a `RerankClient` from arbitrary
    /// `AsyncRead` / `AsyncWrite` halves. Lets sibling-module tests stub
    /// the transport with `tokio::io::duplex`. Not part of the public
    /// API.
    #[doc(hidden)]
    pub fn wrap_for_test(
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> Self {
        Self::wrap(read, write)
    }

    /// Send a `RerankRequest` and read back the single terminal
    /// `RerankResponse` frame (`Rerank` or `Error`). The connection
    /// stays open for the next call.
    ///
    /// On success, `results` arrives already sorted by score descending
    /// and already truncated to `top_n` — the daemon owns both, since
    /// score scales are model-specific and re-deriving the ordering per
    /// consumer invites drift.
    ///
    /// Yields `Err(ClientError::UnexpectedEof)` if the daemon closes the
    /// connection without writing a response (e.g. crashed mid-request).
    /// Callers treat that as equivalent to a `backend_unavailable` error
    /// and apply their own retry policy (ADR 0007 — the daemon never
    /// retries for you).
    pub async fn rerank(&mut self, req: RerankRequest) -> Result<RerankResponse, ClientError> {
        let mut buf = Vec::with_capacity(512);
        serde_json::to_writer(&mut buf, &req)?;
        buf.push(b'\n');

        let mut g = self.inner.lock().await;
        g.write.write_all(&buf).await?;
        g.write.flush().await?;

        let mut line = Vec::with_capacity(512);
        let n = g.read.read_until(b'\n', &mut line).await?;
        if n == 0 {
            return Err(ClientError::UnexpectedEof);
        }
        let resp: RerankResponse = serde_json::from_slice(&line)?;
        Ok(resp)
    }
}

/// Default rerank inference endpoint path, mirroring the daemon's
/// `endpoint::default_rerank_addr`. Returned as a `PathBuf` on Unix and
/// as a pipe-path string on Windows; callers pick by `cfg`.
///
/// Linux fallback chain (same as generation / embed / admin):
/// 1. `${XDG_RUNTIME_DIR}/inferd/infer.rerank.sock`
/// 2. `${HOME}/.inferd/run/infer.rerank.sock`
/// 3. `/tmp/inferd/infer.rerank.sock`
pub fn default_rerank_addr() -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            let mut p = std::path::PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                p.push("inferd");
                p.push("infer.rerank.sock");
                return p;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = std::path::PathBuf::from(home);
            if !p.as_os_str().is_empty() {
                p.push(".inferd");
                p.push("run");
                p.push("infer.rerank.sock");
                return p;
            }
        }
        std::path::PathBuf::from("/tmp/inferd/infer.rerank.sock")
    }
    #[cfg(target_os = "macos")]
    {
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push("infer.rerank.sock");
        p
    }
    #[cfg(windows)]
    {
        std::path::PathBuf::from(r"\\.\pipe\inferd-infer-rerank")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        std::path::PathBuf::from("/tmp/inferd/infer.rerank.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_proto::rerank::{RerankErrorCode, RerankResult, RerankUsage};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    fn sample_request() -> RerankRequest {
        RerankRequest {
            id: "rerank-test".into(),
            query: "how do I bind a unix socket".into(),
            documents: vec![
                "Unrelated text about cheese.".into(),
                "Call bind(2) on an AF_UNIX socket.".into(),
            ],
            top_n: Some(1),
        }
    }

    #[tokio::test]
    async fn rerank_round_trips_a_success_frame() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = RerankClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();

            let frame = serde_json::to_vec(&RerankResponse::Rerank {
                id: "rerank-test".into(),
                results: vec![RerankResult {
                    index: 1,
                    score: 3.75,
                }],
                model: "bge-reranker-v2-m3".into(),
                usage: RerankUsage { input_tokens: 31 },
                backend: "llamacpp".into(),
            })
            .unwrap();
            tx.write_all(&frame).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
        });

        let resp = client.rerank(sample_request()).await.unwrap();
        server.await.unwrap();

        match resp {
            RerankResponse::Rerank {
                results, backend, ..
            } => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].index, 1);
                assert_eq!(results[0].score, 3.75);
                assert_eq!(backend, "llamacpp");
            }
            other => panic!("expected Rerank, got {other:?}"),
        }
    }

    /// Raw logits are the common case for cross-encoders, so a negative
    /// score has to survive the client's deserialise — not just the
    /// proto type's own round-trip test.
    #[tokio::test]
    async fn negative_scores_survive_the_client() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = RerankClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();
            let frame = serde_json::to_vec(&RerankResponse::Rerank {
                id: "rerank-test".into(),
                results: vec![
                    RerankResult {
                        index: 1,
                        score: -0.5,
                    },
                    RerankResult {
                        index: 0,
                        score: -8.25,
                    },
                ],
                model: "m".into(),
                usage: RerankUsage { input_tokens: 2 },
                backend: "llamacpp".into(),
            })
            .unwrap();
            tx.write_all(&frame).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
        });

        let resp = client.rerank(sample_request()).await.unwrap();
        server.await.unwrap();
        match resp {
            RerankResponse::Rerank { results, .. } => {
                assert_eq!(results[0].score, -0.5);
                assert_eq!(results[1].score, -8.25);
            }
            other => panic!("expected Rerank, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rerank_round_trips_an_error_frame() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = RerankClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();

            let frame = serde_json::to_vec(&RerankResponse::Error {
                id: "rerank-test".into(),
                code: RerankErrorCode::RerankUnsupported,
                message: "rerank not supported by this backend".into(),
            })
            .unwrap();
            tx.write_all(&frame).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
        });

        let resp = client.rerank(sample_request()).await.unwrap();
        server.await.unwrap();

        match resp {
            RerankResponse::Error { code, .. } => {
                assert_eq!(code, RerankErrorCode::RerankUnsupported);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unexpected_eof_yields_clienterror() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = RerankClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, _tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();
            // server_side drops here -> EOF on client.
        });

        let result = client.rerank(sample_request()).await;
        server.await.unwrap();
        match result {
            Err(ClientError::UnexpectedEof) => {}
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connection_stays_open_for_a_second_request() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = RerankClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            for i in 0..2 {
                let mut req_line = Vec::new();
                br.read_until(b'\n', &mut req_line).await.unwrap();
                let frame = serde_json::to_vec(&RerankResponse::Rerank {
                    id: format!("r{i}"),
                    results: vec![RerankResult {
                        index: 0,
                        score: 1.0,
                    }],
                    model: "m".into(),
                    usage: RerankUsage { input_tokens: 1 },
                    backend: "mock".into(),
                })
                .unwrap();
                tx.write_all(&frame).await.unwrap();
                tx.write_all(b"\n").await.unwrap();
            }
        });

        for i in 0..2 {
            let req = RerankRequest {
                id: format!("r{i}"),
                query: "q".into(),
                documents: vec!["d".into()],
                ..Default::default()
            };
            let resp = client.rerank(req).await.unwrap();
            assert_eq!(resp.id(), format!("r{i}"));
        }
        server.await.unwrap();
    }
}
