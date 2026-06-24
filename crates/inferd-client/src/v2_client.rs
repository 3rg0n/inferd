//! v2 inference-socket client — the single generation surface.
//!
//! Spec: ADR 0021. As of v0.5 v2 is the only generation socket and
//! rides the length-prefixed, type-tagged framing
//! (`[uvarint len][1 byte type][payload]`, type `0x01` JSON / `0x02`
//! BLOB). Clients pick a transport with `dial_uds` (Unix) or `dial_pipe`
//! (Windows). A request carrying attachments sends the JSON request
//! frame, then per attachment a `BlobDescriptor` JSON frame followed by
//! a BLOB frame with the raw bytes (no base64). 64 MiB per-frame cap;
//! terminal `done` / `error` ends the stream.

use crate::client::ClientError;
use inferd_proto::v2::{BlobDescriptor, RequestV2, ResponseV2, WIRE_VERSION};
use inferd_proto::{FrameType, MAX_FRAME_BYTES};
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_stream::Stream;

/// Stream of `ResponseV2` frames yielded by `ClientV2::generate`.
pub type FrameStreamV2 = Pin<Box<dyn Stream<Item = Result<ResponseV2, ClientError>> + Send>>;

/// v2 inference-socket client.
///
/// Construct via `dial_uds` (Unix) or `dial_pipe` (Windows).
/// Wrap with [`crate::dial_and_wait_ready`] to retry connect during
/// daemon bring-up — the retry helper is generic over the client type.
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
    /// Open a Unix domain socket connection (Unix only). Default
    /// generation path: `${XDG_RUNTIME_DIR}/inferd/inferd.sock` on
    /// Linux, `${TMPDIR}/inferd/inferd.sock` on macOS.
    #[cfg(unix)]
    pub async fn dial_uds(path: &Path) -> Result<Self, ClientError> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self::wrap(Box::new(read), Box::new(write)))
    }

    /// Open a Windows named pipe connection (Windows only). Default
    /// generation path: `\\.\pipe\inferd`.
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
    ///
    /// Writes the request as a length-prefixed JSON frame; then, for
    /// each attachment carrying bytes, a `BlobDescriptor` JSON frame
    /// followed by a BLOB frame with the raw bytes (ADR 0021). Sets
    /// `wire_version` so the daemon can detect a mismatch. The returned
    /// stream completes after a terminal `done` / `error` frame, or
    /// yields `Err(ClientError::UnexpectedEof)` if the daemon closes
    /// mid-stream.
    pub async fn generate(&mut self, mut req: RequestV2) -> Result<FrameStreamV2, ClientError> {
        req.wire_version = WIRE_VERSION;

        // Detach attachment bytes so the request JSON frame carries only
        // metadata; bytes follow as BLOB frames. (Attachment::bytes is
        // already #[serde(skip)], so the JSON wouldn't include them
        // regardless — but we need the raw bytes here to send them.)
        let blobs: Vec<(String, Vec<u8>)> = req
            .attachments
            .iter()
            .filter(|a| !a.bytes().is_empty())
            .map(|a| (a.id().to_string(), a.bytes().to_vec()))
            .collect();

        {
            let mut g = self.inner.lock().await;
            // 1. Request JSON frame.
            write_lp_json_async(&mut g.write, &req).await?;
            // 2. Per attachment: descriptor JSON frame + BLOB frame.
            for (id, bytes) in &blobs {
                let desc = BlobDescriptor::new(id.clone(), bytes.len() as u64);
                write_lp_json_async(&mut g.write, &desc).await?;
                write_lp_blob_async(&mut g.write, bytes).await?;
            }
            g.write.flush().await?;
        }

        let inner = Arc::clone(&self.inner);
        let stream = async_stream::stream! {
            loop {
                let mut g = inner.lock().await;
                let frame = read_lp_raw_async(&mut g.read).await;
                drop(g);
                match frame {
                    Ok(None) => { yield Err(ClientError::UnexpectedEof); return; }
                    Ok(Some((FrameType::Json, payload))) => {
                        match serde_json::from_slice::<ResponseV2>(&payload) {
                            Ok(resp) => {
                                let terminal = resp.is_terminal();
                                yield Ok(resp);
                                if terminal { return; }
                            }
                            Err(e) => { yield Err(ClientError::Decode(e)); return; }
                        }
                    }
                    Ok(Some((FrameType::Blob, _))) => {
                        yield Err(ClientError::MalformedFrame(
                            "daemon sent a BLOB frame on the response stream".into(),
                        ));
                        return;
                    }
                    Err(e) => { yield Err(e); return; }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

// --- length-prefixed framing (async client side) ---------------------------

const MAX_VARINT_BYTES: usize = 5;

async fn write_lp_json_async<W: AsyncWrite + Unpin, T: serde::Serialize>(
    w: &mut W,
    frame: &T,
) -> Result<(), ClientError> {
    let payload = serde_json::to_vec(frame)?;
    write_lp_payload_async(w, FrameType::Json, &payload).await
}

async fn write_lp_blob_async<W: AsyncWrite + Unpin>(
    w: &mut W,
    bytes: &[u8],
) -> Result<(), ClientError> {
    write_lp_payload_async(w, FrameType::Blob, bytes).await
}

async fn write_lp_payload_async<W: AsyncWrite + Unpin>(
    w: &mut W,
    frame_type: FrameType,
    payload: &[u8],
) -> Result<(), ClientError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ClientError::MalformedFrame(format!(
            "outgoing frame {} exceeds {} byte cap",
            payload.len(),
            MAX_FRAME_BYTES
        )));
    }
    let mut prefix = [0u8; MAX_VARINT_BYTES];
    let n = encode_uvarint(payload.len() as u64, &mut prefix);
    w.write_all(&prefix[..n]).await?;
    w.write_all(&[frame_type as u8]).await?;
    w.write_all(payload).await?;
    Ok(())
}

fn encode_uvarint(mut value: u64, out: &mut [u8; MAX_VARINT_BYTES]) -> usize {
    let mut i = 0;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out[i] = byte;
        i += 1;
        if value == 0 {
            return i;
        }
    }
}

/// Read one length-prefixed frame. `Ok(None)` on a clean between-frames
/// EOF; `MalformedFrame` on a bad type byte / non-terminating varint /
/// oversize length / mid-frame EOF.
async fn read_lp_raw_async<R: AsyncRead + Unpin>(
    r: &mut R,
) -> Result<Option<(FrameType, Vec<u8>)>, ClientError> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let mut b = [0u8; 1];
        if r.read(&mut b).await? == 0 {
            if i == 0 {
                return Ok(None);
            }
            return Err(ClientError::MalformedFrame(
                "stream ended mid-length-varint".into(),
            ));
        }
        value |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if i == MAX_VARINT_BYTES - 1 {
            return Err(ClientError::MalformedFrame("length varint too long".into()));
        }
    }
    if value > MAX_FRAME_BYTES as u64 {
        return Err(ClientError::MalformedFrame(format!(
            "incoming frame length {value} exceeds {MAX_FRAME_BYTES} byte cap"
        )));
    }
    let len = value as usize;

    let mut type_byte = [0u8; 1];
    read_exact_async(r, &mut type_byte).await?;
    let frame_type = match type_byte[0] {
        0x01 => FrameType::Json,
        0x02 => FrameType::Blob,
        other => {
            return Err(ClientError::MalformedFrame(format!(
                "unknown frame-type byte 0x{other:02x}"
            )));
        }
    };

    let mut payload = vec![0u8; len];
    read_exact_async(r, &mut payload).await?;
    Ok(Some((frame_type, payload)))
}

async fn read_exact_async<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut [u8],
) -> Result<(), ClientError> {
    match r.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(ClientError::MalformedFrame("stream ended mid-frame".into()))
        }
        Err(e) => Err(ClientError::Io(e)),
    }
}

/// Default generation endpoint path, mirroring the daemon's
/// `endpoint::default_addr` (ADR 0021 — one generation socket on a
/// neutral path, no `-v2` suffix). Returned as `PathBuf` on Unix and as
/// a pipe-path string on Windows; callers pick by `cfg`.
///
/// Linux fallback chain (same as the admin chain):
/// 1. `${XDG_RUNTIME_DIR}/inferd/inferd.sock`
/// 2. `${HOME}/.inferd/run/inferd.sock`
/// 3. `/tmp/inferd/inferd.sock`
pub fn default_v2_addr() -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            let mut p = std::path::PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                p.push("inferd");
                p.push("inferd.sock");
                return p;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = std::path::PathBuf::from(home);
            if !p.as_os_str().is_empty() {
                p.push(".inferd");
                p.push("run");
                p.push("inferd.sock");
                return p;
            }
        }
        std::path::PathBuf::from("/tmp/inferd/inferd.sock")
    }
    #[cfg(target_os = "macos")]
    {
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push("inferd.sock");
        p
    }
    #[cfg(windows)]
    {
        std::path::PathBuf::from(r"\\.\pipe\inferd")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        std::path::PathBuf::from("/tmp/inferd/inferd.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_proto::v2::{
        ContentBlock, ErrorCodeV2, MessageV2, ResponseBlock, RoleV2, StopReasonV2, UsageV2,
    };

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

    /// Read one length-prefixed frame on the test server side, mirroring
    /// the client's reader. Returns `(type_byte, payload)`.
    async fn srv_read_lp<R: AsyncRead + Unpin>(r: &mut R) -> (u8, Vec<u8>) {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let mut b = [0u8; 1];
            r.read_exact(&mut b).await.unwrap();
            value |= u64::from(b[0] & 0x7f) << shift;
            if b[0] & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        let mut tb = [0u8; 1];
        r.read_exact(&mut tb).await.unwrap();
        let mut payload = vec![0u8; value as usize];
        r.read_exact(&mut payload).await.unwrap();
        (tb[0], payload)
    }

    async fn srv_write_lp_json<W: AsyncWrite + Unpin, T: serde::Serialize>(w: &mut W, frame: &T) {
        let payload = serde_json::to_vec(frame).unwrap();
        let mut prefix = [0u8; MAX_VARINT_BYTES];
        let n = encode_uvarint(payload.len() as u64, &mut prefix);
        w.write_all(&prefix[..n]).await.unwrap();
        w.write_all(&[FrameType::Json as u8]).await.unwrap();
        w.write_all(&payload).await.unwrap();
    }

    #[tokio::test]
    async fn generate_streams_frame_then_done() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let mut client = ClientV2::wrap(Box::new(read), Box::new(write));

        let server = tokio::spawn(async move {
            let (mut rx, mut tx) = tokio::io::split(server_side);
            // Read the request frame (length-prefixed JSON).
            let (rtype, payload) = srv_read_lp(&mut rx).await;
            assert_eq!(rtype, FrameType::Json as u8);
            let req: RequestV2 = serde_json::from_slice(&payload).unwrap();
            assert_eq!(req.wire_version, WIRE_VERSION);

            srv_write_lp_json(
                &mut tx,
                &ResponseV2::Frame {
                    id: "v2-test".into(),
                    block: ResponseBlock::Text { delta: "hi".into() },
                },
            )
            .await;
            srv_write_lp_json(
                &mut tx,
                &ResponseV2::Done {
                    id: "v2-test".into(),
                    usage: UsageV2 {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    stop_reason: StopReasonV2::EndTurn,
                    backend: "mock".into(),
                },
            )
            .await;
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
            let (mut rx, _tx) = tokio::io::split(server_side);
            let _ = srv_read_lp(&mut rx).await; // consume request
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
