//! Inference-socket client. NDJSON over UDS / pipe / loopback TCP.

use inferd_proto::{Request, Response};
use std::io;
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_stream::Stream;

/// Errors produced by the inference client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Connection / I/O error against the daemon.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// JSON encode/decode of a wire frame failed.
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
    /// Connection closed before a terminal `done` / `error` frame.
    /// Per `docs/protocol-v1.md`, callers treat this as an error
    /// equivalent to `code: backend_unavailable` and apply their own
    /// retry policy.
    #[error("daemon closed connection before terminal frame")]
    UnexpectedEof,
}

/// Stream of `Response` frames yielded by `Client::generate`.
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<Response, ClientError>> + Send>>;

/// Inference-socket client.
///
/// Construct via `dial_tcp` / `dial_uds` (Unix) / `dial_pipe`
/// (Windows). Wrap with [`crate::dial_and_wait_ready`] to retry
/// connect during daemon bring-up.
pub struct Client {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Inner BufReader / write half don't impl Debug usefully;
        // give callers something to print without leaking the
        // underlying transport state.
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

struct Inner {
    write: Box<dyn AsyncWrite + Send + Unpin>,
    read: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

impl Client {
    /// Open a TCP connection to `addr` (e.g. `"127.0.0.1:47321"`).
    pub async fn dial_tcp(addr: &str) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(addr).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Unix domain socket connection (Unix only).
    #[cfg(unix)]
    pub async fn dial_uds(path: &Path) -> Result<Self, ClientError> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Windows named pipe connection (Windows only).
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

    /// Test-only constructor: build a `Client` from arbitrary
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

    /// Send a `Request` and return a stream of response frames.
    /// The stream completes after a terminal `done` or `error` frame
    /// (or yields `Err(ClientError::UnexpectedEof)` if the daemon
    /// closes the connection mid-stream).
    pub async fn generate(&mut self, req: Request) -> Result<FrameStream, ClientError> {
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

                match serde_json::from_slice::<Response>(&line) {
                    Ok(resp) => {
                        let terminal = matches!(
                            &resp,
                            Response::Done { .. } | Response::Error { .. }
                        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_proto::{ErrorCode, Message, Role, StopReason, Usage};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    fn sample_request() -> Request {
        Request {
            id: "test".into(),
            messages: vec![Message {
                role: Role::User,
                content: "hello".into(),
            }],
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            stream: None,
            image_token_budget: None,
            grammar: String::new(),
        }
    }

    /// Wrap an in-memory duplex pipe as a Client and drive a
    /// canned-server scenario from the other end of the pipe.
    /// Avoids a real socket so unit tests stay platform-agnostic.
    #[tokio::test]
    async fn generate_streams_token_then_done() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = Client::wrap(Box::new(read), Box::new(write));

        // Server task: read one Request frame, write Token + Done.
        let server = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server_side);
            let mut br = tokio::io::BufReader::new(rx);
            let mut req_line = Vec::new();
            br.read_until(b'\n', &mut req_line).await.unwrap();

            // Token frame.
            let token = serde_json::to_vec(&Response::Token {
                id: "test".into(),
                content: "hi".into(),
            })
            .unwrap();
            tx.write_all(&token).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
            // Done frame.
            let done = serde_json::to_vec(&Response::Done {
                id: "test".into(),
                content: "hi".into(),
                usage: Usage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                },
                stop_reason: StopReason::End,
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
            Response::Token { content, .. } => assert_eq!(content, "hi"),
            other => panic!("frame[0]: {other:?}"),
        }
        match frames[1].as_ref().unwrap() {
            Response::Done { backend, .. } => assert_eq!(backend, "mock"),
            other => panic!("frame[1]: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unexpected_eof_yields_clienterror() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = Client::wrap(Box::new(read), Box::new(write));

        // Server: read request, immediately drop without writing.
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
    fn error_code_round_trips() {
        // Sanity check: the re-export from inferd-proto behaves
        // the same as a direct inferd_proto::ErrorCode.
        let frame = Response::Error {
            id: "x".into(),
            code: ErrorCode::QueueFull,
            message: "queue full".into(),
        };
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains(r#""code":"queue_full""#));
    }
}
