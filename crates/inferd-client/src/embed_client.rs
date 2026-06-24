//! Embed-socket client. NDJSON over UDS / named pipe.
//!
//! Spec: ADR 0017. The embed socket is the *third* inferd surface
//! (v2, embed), each on its own path. Construct an `EmbedClient`
//! with `dial_uds` (Unix) or `dial_pipe` (Windows), then call `embed`
//! per request. The connection is long-lived: send a request, receive
//! one terminal frame, send the next request — there is no streaming
//! since an embedding is a complete vector.

use crate::client::ClientError;
use inferd_proto::embed::{EmbedRequest, EmbedResponse};
#[cfg(unix)]
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Embed-socket client.
///
/// Construct via `dial_uds` (Unix) or `dial_pipe` (Windows).
/// Wrap with [`crate::dial_and_wait_ready`] to retry connect during
/// daemon bring-up — the retry helper is generic over the client type.
pub struct EmbedClient {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for EmbedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbedClient").finish_non_exhaustive()
    }
}

struct Inner {
    write: Box<dyn AsyncWrite + Send + Unpin>,
    read: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

impl EmbedClient {
    /// Open a Unix domain socket connection (Unix only). Default embed
    /// path: `${XDG_RUNTIME_DIR}/inferd/infer.embed.sock` on Linux,
    /// `${TMPDIR}/inferd/infer.embed.sock` on macOS.
    #[cfg(unix)]
    pub async fn dial_uds(path: &Path) -> Result<Self, ClientError> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Windows named pipe connection (Windows only). Default
    /// embed path: `\\.\pipe\inferd-infer-embed`.
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

    /// Test-only constructor: build an `EmbedClient` from arbitrary
    /// `AsyncRead` / `AsyncWrite` halves. Lets sibling-module tests
    /// stub the transport with `tokio::io::duplex`. Not part of the
    /// public API.
    #[doc(hidden)]
    pub fn wrap_for_test(
        read: Box<dyn AsyncRead + Send + Unpin>,
        write: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> Self {
        Self::wrap(read, write)
    }

    /// Send an `EmbedRequest` and read back the single terminal
    /// `EmbedResponse` frame (`Embeddings` or `Error`). The connection
    /// stays open for the next call.
    ///
    /// Yields `Err(ClientError::UnexpectedEof)` if the daemon closes
    /// the connection without writing a response (e.g. crashed mid-
    /// request). Per `docs/protocol-v1.md`, callers treat this as
    /// equivalent to a `backend_unavailable` error and apply their own
    /// retry policy.
    pub async fn embed(&mut self, req: EmbedRequest) -> Result<EmbedResponse, ClientError> {
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
        let resp: EmbedResponse = serde_json::from_slice(&line)?;
        Ok(resp)
    }
}

/// Default embed inference endpoint path, mirroring the daemon's
/// `endpoint::default_embed_addr`. Returned as a `PathBuf` on Unix
/// and as a pipe-path string on Windows; callers pick by `cfg`.
///
/// Linux fallback chain (same as v2 / admin):
/// 1. `${XDG_RUNTIME_DIR}/inferd/infer.embed.sock`
/// 2. `${HOME}/.inferd/run/infer.embed.sock`
/// 3. `/tmp/inferd/infer.embed.sock`
pub fn default_embed_addr() -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            let mut p = std::path::PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                p.push("inferd");
                p.push("infer.embed.sock");
                return p;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = std::path::PathBuf::from(home);
            if !p.as_os_str().is_empty() {
                p.push(".inferd");
                p.push("run");
                p.push("infer.embed.sock");
                return p;
            }
        }
        std::path::PathBuf::from("/tmp/inferd/infer.embed.sock")
    }
    #[cfg(target_os = "macos")]
    {
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push("infer.embed.sock");
        p
    }
    #[cfg(windows)]
    {
        std::path::PathBuf::from(r"\\.\pipe\inferd-infer-embed")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        std::path::PathBuf::from("/tmp/inferd/infer.embed.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_proto::embed::{EmbedErrorCode, EmbedTask, EmbedUsage};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    fn sample_request() -> EmbedRequest {
        EmbedRequest {
            id: "embed-test".into(),
            input: vec!["hello".into(), "world".into()],
            dimensions: Some(128),
            task: Some(EmbedTask::RetrievalDocument),
        }
    }

    #[tokio::test]
    async fn embed_round_trips_a_success_frame() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = EmbedClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();

            let frame = serde_json::to_vec(&EmbedResponse::Embeddings {
                id: "embed-test".into(),
                embeddings: vec![vec![0.1, 0.2], vec![0.3, 0.4]],
                dimensions: 128,
                model: "embeddinggemma-300m".into(),
                usage: EmbedUsage { input_tokens: 4 },
                backend: "llamacpp".into(),
            })
            .unwrap();
            tx.write_all(&frame).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
        });

        let resp = client.embed(sample_request()).await.unwrap();
        server.await.unwrap();

        match resp {
            EmbedResponse::Embeddings {
                embeddings,
                dimensions,
                backend,
                ..
            } => {
                assert_eq!(embeddings.len(), 2);
                assert_eq!(dimensions, 128);
                assert_eq!(backend, "llamacpp");
            }
            other => panic!("expected Embeddings, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn embed_round_trips_an_error_frame() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = EmbedClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();

            let frame = serde_json::to_vec(&EmbedResponse::Error {
                id: "embed-test".into(),
                code: EmbedErrorCode::InvalidRequest,
                message: "dimensions=999 not supported".into(),
            })
            .unwrap();
            tx.write_all(&frame).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
        });

        let resp = client.embed(sample_request()).await.unwrap();
        server.await.unwrap();

        match resp {
            EmbedResponse::Error { code, .. } => {
                assert_eq!(code, EmbedErrorCode::InvalidRequest);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unexpected_eof_yields_clienterror() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = EmbedClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, _tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();
            // server_side drops here -> EOF on client.
        });

        let result = client.embed(sample_request()).await;
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
        let mut client = EmbedClient::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            for i in 0..2 {
                let mut req_line = Vec::new();
                br.read_until(b'\n', &mut req_line).await.unwrap();
                let frame = serde_json::to_vec(&EmbedResponse::Embeddings {
                    id: format!("r{i}"),
                    embeddings: vec![vec![0.0]],
                    dimensions: 1,
                    model: "m".into(),
                    usage: EmbedUsage { input_tokens: 1 },
                    backend: "mock".into(),
                })
                .unwrap();
                tx.write_all(&frame).await.unwrap();
                tx.write_all(b"\n").await.unwrap();
            }
        });

        for i in 0..2 {
            let req = EmbedRequest {
                id: format!("r{i}"),
                input: vec!["x".into()],
                ..Default::default()
            };
            let resp = client.embed(req).await.unwrap();
            assert_eq!(resp.id(), format!("r{i}"));
        }
        server.await.unwrap();
    }
}
