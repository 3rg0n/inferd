//! v2 connection lifecycle — the single generation surface (ADR 0021).
//!
//! As of v0.4 v2 is the *only* generation socket (v1 was folded in and
//! removed) and rides the length-prefixed, type-tagged framing
//! (`[uvarint len][1 byte type][payload]`, type `0x01` JSON / `0x02`
//! BLOB) rather than newline-delimited JSON. Attachment bytes travel
//! out-of-band in BLOB frames.
//!
//! Per request:
//!   1. Read the JSON request frame, parse as `RequestV2`.
//!   2. Reject if `wire_version != WIRE_VERSION` (loud mismatch).
//!   3. For each attachment, read a `BlobDescriptor` JSON frame + its
//!      BLOB frame; install the raw bytes into the matching attachment
//!      by id (`Attachment::set_bytes`). No base64.
//!   4. `RequestV2::resolve()` — structural validation.
//!   5. Admission gate (one active generation, bounded queue).
//!   6. Dispatch through the router; require the chosen backend's
//!      `capabilities().v2`.
//!   7. `backend.generate_v2(resolved)`; pre-stream errors map to v2
//!      error codes, mid-stream failure (no Done) → `BackendUnavailable`.
//!   8. Stream `TokenEventV2`s, translating each to `ResponseV2::Frame`
//!      / `Done` (written as length-prefixed JSON frames).

use crate::endpoint::Connection;
use crate::peercred::PeerIdentity;
use crate::queue::SubmitError;
use crate::router::{Router, RouterError};
use inferd_engine::{GenerateError, TokenEventV2};
use inferd_proto::ProtoError;
use inferd_proto::v2::{
    Attachment, BlobDescriptor, BlobDescriptorTag, ErrorCodeV2, MAX_ATTACHMENT_BYTES_PER_REQUEST,
    MAX_ATTACHMENTS_PER_REQUEST, RequestV2, ResponseBlock, ResponseV2, ToolChoice, WIRE_VERSION,
};
use inferd_proto::{FrameType, MAX_FRAME_BYTES, decode_json_payload, write_lp_json};
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

/// Per-accept policy for v2 connections — the same type the embed
/// surface uses, because both share one admission gate: one slot is one
/// slot regardless of which surface asked for it.
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
    // THREAT_MODEL F-17: every response write is bounded, because writes
    // downstream of the admission gate happen while the permit is held.
    let write_timeout = ctx.write_timeout;

    loop {
        // 1. Request JSON frame.
        let mut request: RequestV2 = match read_json_frame::<_, RequestV2>(&mut reader).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(ProtoError::Io(e)) => return Err(e),
            Err(e) => {
                // Framing or decode error before we even have a request
                // id — report against an empty id and close, since the
                // byte stream is no longer trustworthy.
                let resp = ResponseV2::Error {
                    id: String::new(),
                    code: error_code_for(&e),
                    message: e.to_string(),
                };
                write_response_v2(&writer, &resp, write_timeout).await?;
                return Ok(());
            }
        };

        let id = request.id.clone();

        // 2. Wire-version gate (ADR 0021). Fail loudly on mismatch.
        if request.wire_version != WIRE_VERSION {
            let resp = ResponseV2::Error {
                id,
                code: ErrorCodeV2::WireVersionUnsupported,
                message: format!(
                    "unsupported wire_version {}: this daemon speaks wire_version {}",
                    request.wire_version, WIRE_VERSION
                ),
            };
            write_response_v2(&writer, &resp, write_timeout).await?;
            // A version mismatch means the peer's framing assumptions may
            // differ from ours; don't try to resync on this connection.
            return Ok(());
        }

        // 3. Attachment BLOBs. Each attachment's raw bytes arrive as a
        //    BlobDescriptor JSON frame followed by a BLOB frame, in the
        //    order the attachments appear in the request. Install them by
        //    id. A framing/correlation failure here closes the connection.
        if let Err(e) = read_attachment_blobs(&mut reader, &mut request.attachments).await {
            let resp = ResponseV2::Error {
                id: request.id.clone(),
                code: error_code_for(&e),
                message: e.to_string(),
            };
            write_response_v2(&writer, &resp, write_timeout).await?;
            return Ok(());
        }

        // 4. Structural validation. `resolve` consumes the request, so
        //    capture the id first for the error path.
        let req_id_for_resolve = request.id.clone();
        let resolved = match request.resolve() {
            Ok(r) => r,
            Err(e) => {
                let resp = ResponseV2::Error {
                    id: req_id_for_resolve,
                    code: ErrorCodeV2::InvalidRequest,
                    message: e.to_string(),
                };
                write_response_v2(&writer, &resp, write_timeout).await?;
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
                write_response_v2(&writer, &resp, write_timeout).await?;
                continue;
            }
            Some(Err(SubmitError::Closed)) => {
                let resp = ResponseV2::Error {
                    id: resolved.id.clone(),
                    code: ErrorCodeV2::BackendUnavailable,
                    message: "admission closed".into(),
                };
                write_response_v2(&writer, &resp, write_timeout).await?;
                return Ok(());
            }
        };

        // Dispatch through the router.
        let dispatch = match router.dispatch() {
            Ok(d) => d,
            Err(RouterError::NoBackends) | Err(RouterError::NoneAvailable) => {
                let resp = ResponseV2::Error {
                    id: resolved.id.clone(),
                    code: ErrorCodeV2::BackendUnavailable,
                    message: "no backend available".into(),
                };
                write_response_v2(&writer, &resp, write_timeout).await?;
                continue;
            }
        };
        let backend_name = dispatch.name.clone();
        let backend = dispatch.backend;

        // Backends that don't advertise v2 capability are not given a
        // generate_v2 call — the trait's default impl would also
        // refuse, but checking up front lets us emit a clearer error
        // and avoids paying for a method-dispatch round-trip just to
        // surface "not supported".
        if !backend.capabilities().v2 {
            let resp = ResponseV2::Error {
                id: resolved.id.clone(),
                code: ErrorCodeV2::Internal,
                message: format!("backend {backend_name:?} does not advertise v2 capability"),
            };
            write_response_v2(&writer, &resp, write_timeout).await?;
            continue;
        }

        let req_id = resolved.id.clone();
        let n_attachments = resolved.attachments.len();
        let n_tools = resolved.tools.len();
        // Whether this request demanded a call, so the terminal frame
        // can report that none arrived (ADR 0029). Read here because
        // `resolved` moves into `generate_v2` below.
        //
        // Computed in the relay rather than per-backend on purpose: the
        // answer is "did a ToolUse cross this stream", which the relay
        // already sees for every adapter. Threading it through
        // `TokenEventV2::Done` instead would make each of the four
        // backends responsible for getting the same bookkeeping right,
        // and a backend that forgot would silently report "satisfied".
        let tool_choice_required = resolved.tool_choice == Some(ToolChoice::Required);

        let mut stream = match backend.generate_v2(resolved).await {
            Ok(s) => s,
            Err(e) => {
                let (code, message, is_backend_failure) = match e {
                    GenerateError::InvalidRequest(m) => (ErrorCodeV2::InvalidRequest, m, false),
                    GenerateError::NotReady => (
                        ErrorCodeV2::BackendUnavailable,
                        "backend not ready".into(),
                        true,
                    ),
                    GenerateError::Unavailable(m) => (ErrorCodeV2::BackendUnavailable, m, true),
                    GenerateError::Internal(m) => (ErrorCodeV2::Internal, m, true),
                };
                if is_backend_failure {
                    router.record_failure(&backend_name);
                }
                let resp = ResponseV2::Error {
                    id: req_id,
                    code,
                    message,
                };
                write_response_v2(&writer, &resp, write_timeout).await?;
                continue;
            }
        };

        let mut terminal_emitted = false;
        let mut saw_tool_use = false;
        while let Some(ev) = stream.next().await {
            match ev {
                TokenEventV2::Text(delta) => {
                    let frame = ResponseV2::Frame {
                        id: req_id.clone(),
                        block: ResponseBlock::Text { delta },
                    };
                    write_response_v2(&writer, &frame, write_timeout).await?;
                }
                TokenEventV2::Thinking(delta) => {
                    let frame = ResponseV2::Frame {
                        id: req_id.clone(),
                        block: ResponseBlock::Thinking { delta },
                    };
                    write_response_v2(&writer, &frame, write_timeout).await?;
                }
                TokenEventV2::ToolUse {
                    tool_call_id,
                    name,
                    input,
                } => {
                    saw_tool_use = true;
                    let frame = ResponseV2::Frame {
                        id: req_id.clone(),
                        block: ResponseBlock::ToolUse {
                            tool_call_id,
                            name,
                            input,
                        },
                    };
                    write_response_v2(&writer, &frame, write_timeout).await?;
                }
                TokenEventV2::Done { stop_reason, usage } => {
                    // `required` promises the turn cannot *end* without
                    // a call, not that one arrives — a declining model
                    // runs to `max_tokens` instead (ADR 0029). Say so,
                    // rather than leaving the caller to infer it from a
                    // `max_tokens` that also means "ran out of room".
                    let tool_choice_unsatisfied = tool_choice_required && !saw_tool_use;
                    let frame = ResponseV2::Done {
                        id: req_id.clone(),
                        usage,
                        stop_reason,
                        backend: backend_name.clone(),
                        tool_choice_unsatisfied,
                    };
                    write_response_v2(&writer, &frame, write_timeout).await?;
                    router.record_success(&backend_name);
                    info!(
                        target: "inferd_daemon::activity",
                        req_id = %req_id,
                        backend = %backend_name,
                        wire_version = "v2",
                        stop_reason = ?stop_reason,
                        tool_choice_unsatisfied = tool_choice_unsatisfied,
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
            router.record_failure(&backend_name);
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
            write_response_v2(&writer, &frame, write_timeout).await?;
        }
    }
}

fn error_code_for(e: &ProtoError) -> ErrorCodeV2 {
    match e {
        ProtoError::FrameTooLarge => ErrorCodeV2::FrameTooLarge,
        ProtoError::Decode(_) | ProtoError::InvalidRequest(_) | ProtoError::MalformedFrame(_) => {
            ErrorCodeV2::InvalidRequest
        }
        ProtoError::Io(_) => ErrorCodeV2::Internal,
    }
}

// --- length-prefixed framing (async) ---------------------------------------
//
// Async mirror of inferd_proto's sync `read_lp_frame`. The proto codec
// is `std::io::BufRead`-based; the daemon's transport is tokio
// `AsyncRead`, so we re-implement the same wire grammar here:
// `[uvarint payload_len][1 byte type][payload]`, 64 MiB cap enforced on
// `payload_len` before the payload is read.

const MAX_VARINT_BYTES: usize = 5;

/// Read one length-prefixed frame. `Ok(None)` on a clean between-frames
/// EOF (peer closed). Errors mirror the sync codec: `FrameTooLarge` for
/// an oversize length, `MalformedFrame` for an unknown type byte / a
/// non-terminating length varint / mid-frame EOF, `Io` for transport
/// errors.
async fn read_lp_raw<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<(FrameType, Vec<u8>)>, ProtoError> {
    // payload_len (LEB128 varint).
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let mut b = [0u8; 1];
        if reader.read(&mut b).await? == 0 {
            if i == 0 {
                return Ok(None); // clean EOF between frames
            }
            return Err(ProtoError::MalformedFrame(
                "stream ended mid-length-varint".into(),
            ));
        }
        value |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if i == MAX_VARINT_BYTES - 1 {
            return Err(ProtoError::MalformedFrame(format!(
                "length varint exceeded {MAX_VARINT_BYTES} bytes"
            )));
        }
    }
    if value > MAX_FRAME_BYTES as u64 {
        return Err(ProtoError::FrameTooLarge);
    }
    let payload_len = value as usize;

    // frame_type (1 byte).
    let mut type_byte = [0u8; 1];
    read_exact_or_malformed(reader, &mut type_byte, "frame-type byte").await?;
    let frame_type = match type_byte[0] {
        0x01 => FrameType::Json,
        0x02 => FrameType::Blob,
        other => {
            return Err(ProtoError::MalformedFrame(format!(
                "unknown frame-type byte 0x{other:02x}"
            )));
        }
    };

    // payload.
    let mut payload = vec![0u8; payload_len];
    read_exact_or_malformed(reader, &mut payload, "frame payload").await?;
    Ok(Some((frame_type, payload)))
}

async fn read_exact_or_malformed<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut [u8],
    what: &str,
) -> Result<(), ProtoError> {
    match reader.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(ProtoError::MalformedFrame(
            format!("stream ended mid-frame reading {what}"),
        )),
        Err(e) => Err(ProtoError::Io(e)),
    }
}

/// Read one length-prefixed JSON frame and decode it as `T`. Errors if
/// the next frame is a BLOB where a JSON control frame was expected.
async fn read_json_frame<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> Result<Option<T>, ProtoError> {
    match read_lp_raw(reader).await? {
        None => Ok(None),
        Some((FrameType::Json, payload)) => decode_json_payload::<T>(&payload).map(Some),
        Some((FrameType::Blob, _)) => Err(ProtoError::MalformedFrame(
            "expected a JSON control frame, got a BLOB frame".into(),
        )),
    }
}

/// For each attachment in `attachments`, read its `BlobDescriptor` JSON
/// frame followed by its BLOB frame and install the raw bytes by id
/// (ADR 0021). Descriptors must reference attachment ids present in the
/// request, the BLOB length must match the descriptor, and each
/// attachment must receive exactly one BLOB.
///
/// Per-request bounds (THREAT_MODEL F-1). The 64 MiB frame cap bounds one
/// frame; these bound the whole request, because each declared attachment
/// entitles the peer to one more BLOB frame:
///   - at most `MAX_ATTACHMENTS_PER_REQUEST` attachments, checked before
///     the first frame is read;
///   - at most `MAX_ATTACHMENT_BYTES_PER_REQUEST` summed across all BLOBs,
///     charged against each descriptor's *declared* `len` before its
///     payload is read, so an over-budget request costs no heap.
async fn read_attachment_blobs<R: AsyncRead + Unpin>(
    reader: &mut R,
    attachments: &mut [Attachment],
) -> Result<(), ProtoError> {
    read_attachment_blobs_bounded(
        reader,
        attachments,
        MAX_ATTACHMENTS_PER_REQUEST,
        MAX_ATTACHMENT_BYTES_PER_REQUEST,
    )
    .await
}

/// [`read_attachment_blobs`] with the per-request bounds passed in.
/// Split out so the bounds logic is testable without materialising the
/// ≥128 MiB of real frames the production constants would require.
async fn read_attachment_blobs_bounded<R: AsyncRead + Unpin>(
    reader: &mut R,
    attachments: &mut [Attachment],
    max_attachments: usize,
    max_total_bytes: u64,
) -> Result<(), ProtoError> {
    let expected = attachments.len();
    if expected > max_attachments {
        return Err(ProtoError::InvalidRequest(format!(
            "request declares {expected} attachments; at most {max_attachments} allowed"
        )));
    }
    let mut budget_remaining = max_total_bytes;
    for _ in 0..expected {
        // Descriptor (JSON frame).
        let desc: BlobDescriptor = match read_json_frame(reader).await? {
            Some(d) => d,
            None => {
                return Err(ProtoError::MalformedFrame(
                    "stream ended before all attachment BLOBs were sent".into(),
                ));
            }
        };
        if !matches!(desc.frame_kind, BlobDescriptorTag::AttachmentBlob) {
            return Err(ProtoError::MalformedFrame(
                "expected an attachment_blob descriptor".into(),
            ));
        }
        if desc.len > MAX_FRAME_BYTES as u64 || desc.len > budget_remaining {
            return Err(ProtoError::FrameTooLarge);
        }
        budget_remaining -= desc.len;

        // BLOB frame.
        let (ftype, bytes) = match read_lp_raw(reader).await? {
            Some(v) => v,
            None => {
                return Err(ProtoError::MalformedFrame(
                    "stream ended before the attachment BLOB frame".into(),
                ));
            }
        };
        if !matches!(ftype, FrameType::Blob) {
            return Err(ProtoError::MalformedFrame(
                "expected a BLOB frame after its descriptor".into(),
            ));
        }
        if bytes.len() as u64 != desc.len {
            return Err(ProtoError::MalformedFrame(format!(
                "attachment {:?}: BLOB length {} != descriptor len {}",
                desc.attachment_id,
                bytes.len(),
                desc.len
            )));
        }

        // Correlate by id and install. Reject a descriptor naming an id
        // not in the request, or a second BLOB for an already-filled
        // attachment.
        let target = attachments
            .iter_mut()
            .find(|a| a.id() == desc.attachment_id)
            .ok_or_else(|| {
                ProtoError::MalformedFrame(format!(
                    "BLOB descriptor names unknown attachment id {:?}",
                    desc.attachment_id
                ))
            })?;
        if !target.bytes().is_empty() {
            return Err(ProtoError::MalformedFrame(format!(
                "attachment {:?} received more than one BLOB",
                desc.attachment_id
            )));
        }
        target.set_bytes(bytes);
    }
    Ok(())
}

/// Write one length-prefixed JSON response frame, bounded by
/// `timeout` (THREAT_MODEL F-17).
///
/// The bound covers the lock acquisition as well as the write and flush:
/// a peer wedged inside `write_all` holds the writer mutex, so an
/// unbounded `lock().await` here would stall just as long as an unbounded
/// write. On expiry the frame is abandoned and the error propagates,
/// which drops the connection and — crucially — releases the caller's
/// admission permit.
async fn write_response_v2<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    resp: &ResponseV2,
    timeout: Option<std::time::Duration>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(512);
    write_lp_json(&mut buf, resp)
        .map_err(|e| io::Error::other(format!("serialise v2 response: {e}")))?;
    let write = async {
        let mut guard = writer.lock().await;
        guard.write_all(&buf).await?;
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
                            sid: None,
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
                            sid: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_proto::write_lp_blob;

    /// Build the descriptor+BLOB byte stream for `sizes`, as a well-behaved
    /// producer would. Each entry names attachment `a{i}`.
    fn blob_stream(sizes: &[usize]) -> Vec<u8> {
        let mut wire = Vec::new();
        for (i, &len) in sizes.iter().enumerate() {
            write_lp_json(&mut wire, &BlobDescriptor::new(format!("a{i}"), len as u64))
                .expect("descriptor");
            write_lp_blob(&mut wire, &vec![0u8; len]).expect("blob");
        }
        wire
    }

    fn image_attachments(n: usize) -> Vec<Attachment> {
        (0..n)
            .map(|i| Attachment::Image {
                id: format!("a{i}"),
                width: 1,
                height: 1,
                bytes: Vec::new(),
            })
            .collect()
    }

    /// THREAT_MODEL F-1: the per-frame cap bounds each BLOB at 64 MiB, but
    /// nothing bounded their *sum*. A request declaring N attachments could
    /// direct N × 64 MiB of reads into daemon heap. The budget must be
    /// charged against the *declared* descriptor `len`, i.e. refused before
    /// the BLOB payload is read at all.
    #[tokio::test]
    async fn attachment_blobs_reject_over_budget_aggregate() {
        // Two attachments whose declared lengths together exceed the budget.
        // Only the first is actually written: the reader must refuse on the
        // second descriptor without ever waiting for its payload.
        let mut wire = Vec::new();
        write_lp_json(&mut wire, &BlobDescriptor::new("a0", 6)).expect("descriptor");
        write_lp_blob(&mut wire, &[0u8; 6]).expect("blob");
        write_lp_json(&mut wire, &BlobDescriptor::new("a1", 6)).expect("descriptor");
        // a1's BLOB deliberately absent — the reader must not get that far.

        let mut attachments = image_attachments(2);
        let err = read_attachment_blobs_bounded(&mut wire.as_slice(), &mut attachments, 32, 10)
            .await
            .expect_err("aggregate attachment bytes must be bounded");
        assert!(
            matches!(err, ProtoError::FrameTooLarge),
            "unexpected error: {err}"
        );
    }

    /// The count cap must be refused before the first frame is read, so a
    /// flooded attachment table costs one request frame and nothing else.
    #[tokio::test]
    async fn attachment_blobs_reject_over_count_before_reading() {
        let mut attachments = image_attachments(3);
        let err = read_attachment_blobs_bounded(&mut [].as_slice(), &mut attachments, 2, u64::MAX)
            .await
            .expect_err("attachment count must be bounded");
        assert!(
            matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("attachments")),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn attachment_blobs_accept_within_bounds() {
        let wire = blob_stream(&[3, 4]);
        let mut attachments = image_attachments(2);
        read_attachment_blobs_bounded(&mut wire.as_slice(), &mut attachments, 32, 7)
            .await
            .expect("within-budget attachments must be accepted");
        assert_eq!(attachments[0].bytes(), &[0u8; 3]);
        assert_eq!(attachments[1].bytes(), &[0u8; 4]);
    }
}
