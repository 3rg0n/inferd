//! Shared length-prefixed (ADR 0021) framing + v2 helpers for the
//! daemon integration tests.
//!
//! As of v0.4 all generation traffic rides the single v2 socket with
//! `[uvarint payload_len][1 byte type][payload]` framing (type `0x01`
//! JSON / `0x02` BLOB), replacing the newline-delimited JSON the
//! pre-v0.4 tests used. These helpers mirror the wire grammar the
//! daemon's `lifecycle_v2` and the `inferd-client` `ClientV2` speak so
//! each test file doesn't re-hand-roll the codec.
//!
//! `#![allow(dead_code)]` because every test binary links the whole
//! module but uses only the subset it needs.

#![allow(dead_code)]

use inferd_proto::FrameType;
use inferd_proto::v2::{
    Attachment, BlobDescriptor, ContentBlock, MessageV2, RequestV2, ResponseV2, RoleV2,
    WIRE_VERSION,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_VARINT_BYTES: usize = 5;

/// LEB128-encode `value` into `out`, returning the byte count written.
pub fn encode_uvarint(mut value: u64, out: &mut [u8; MAX_VARINT_BYTES]) -> usize {
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

/// Write one length-prefixed frame with an arbitrary type byte. Used by
/// the framing helpers below and directly by tests that need to send a
/// deliberately-malformed JSON payload.
pub async fn write_lp_payload<W: AsyncWrite + Unpin>(w: &mut W, frame_type: u8, payload: &[u8]) {
    let mut prefix = [0u8; MAX_VARINT_BYTES];
    let n = encode_uvarint(payload.len() as u64, &mut prefix);
    w.write_all(&prefix[..n])
        .await
        .expect("write length prefix");
    w.write_all(&[frame_type]).await.expect("write type byte");
    w.write_all(payload).await.expect("write payload");
}

/// Write a length-prefixed JSON frame.
pub async fn write_lp_json<W: AsyncWrite + Unpin, T: serde::Serialize>(w: &mut W, frame: &T) {
    let payload = serde_json::to_vec(frame).expect("serialise json frame");
    write_lp_payload(w, FrameType::Json as u8, &payload).await;
}

/// Write a length-prefixed BLOB frame.
pub async fn write_lp_blob<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) {
    write_lp_payload(w, FrameType::Blob as u8, bytes).await;
}

/// Write a full v2 request: the JSON request frame (with `wire_version`
/// stamped), then for each attachment carrying bytes a `BlobDescriptor`
/// JSON frame followed by its BLOB frame (ADR 0021). Flushes at the end.
pub async fn write_request<W: AsyncWrite + Unpin>(w: &mut W, req: &RequestV2) {
    let mut req = req.clone();
    req.wire_version = WIRE_VERSION;

    let blobs: Vec<(String, Vec<u8>)> = req
        .attachments
        .iter()
        .filter(|a| !a.bytes().is_empty())
        .map(|a| (a.id().to_string(), a.bytes().to_vec()))
        .collect();

    write_lp_json(w, &req).await;
    for (id, bytes) in &blobs {
        let desc = BlobDescriptor::new(id.clone(), bytes.len() as u64);
        write_lp_json(w, &desc).await;
        write_lp_blob(w, bytes).await;
    }
    w.flush().await.expect("flush request");
}

/// Write a length-prefixed JSON auth frame (`{"type":"auth","key":...}`)
/// for the TCP F-8 first-frame auth path.
pub async fn write_auth<W: AsyncWrite + Unpin>(w: &mut W, key: &str) {
    let frame = serde_json::json!({ "type": "auth", "key": key });
    write_lp_json(w, &frame).await;
    w.flush().await.expect("flush auth");
}

/// Read one length-prefixed frame. `None` on a clean between-frames EOF.
/// Returns `(type_byte, payload)`. Panics on a malformed/truncated frame
/// so a test that expected a clean frame fails loudly.
pub async fn read_lp_frame<R: AsyncRead + Unpin>(r: &mut R) -> Option<(u8, Vec<u8>)> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let mut b = [0u8; 1];
        match r.read(&mut b).await.expect("read length byte") {
            0 if i == 0 => return None, // clean EOF between frames
            0 => panic!("stream ended mid-length-varint"),
            _ => {}
        }
        value |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    let mut type_byte = [0u8; 1];
    r.read_exact(&mut type_byte).await.expect("read type byte");
    let mut payload = vec![0u8; value as usize];
    r.read_exact(&mut payload).await.expect("read payload");
    Some((type_byte[0], payload))
}

/// Read JSON frames, decoding each as `ResponseV2`, until a terminal
/// (`Done` / `Error`) frame or EOF. A BLOB frame on the response stream
/// is a protocol violation and panics.
pub async fn collect_frames<R: AsyncRead + Unpin>(r: &mut R) -> Vec<ResponseV2> {
    let mut frames = Vec::new();
    while let Some((type_byte, payload)) = read_lp_frame(r).await {
        assert_eq!(
            type_byte,
            FrameType::Json as u8,
            "daemon emitted a non-JSON frame on the response stream"
        );
        let resp: ResponseV2 = serde_json::from_slice(&payload).expect("decode v2 response frame");
        let terminal = resp.is_terminal();
        frames.push(resp);
        if terminal {
            break;
        }
    }
    frames
}

/// Build a minimal single-text-block v2 request.
pub fn text_request(id: &str, text: &str) -> RequestV2 {
    RequestV2 {
        wire_version: WIRE_VERSION,
        id: id.into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }],
        ..Default::default()
    }
}

/// Construct an image attachment carrying raw RGB bytes, for multimodal
/// dispatch tests. The daemon routes the BLOB by id; the mock backend
/// ignores the pixels.
pub fn image_attachment(id: &str, width: u32, height: u32, bytes: Vec<u8>) -> Attachment {
    let mut a = Attachment::Image {
        id: id.into(),
        width,
        height,
        bytes: Vec::new(),
    };
    a.set_bytes(bytes);
    a
}
