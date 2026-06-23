//! NDJSON framing — bounded reader and unbuffered writer.
//!
//! See `docs/protocol-v1.md` §Framing and THREAT_MODEL.md F-1.

use crate::error::ProtoError;
use serde::{Serialize, de::DeserializeOwned};
use std::io::{BufRead, Write};

/// Hard cap on a single NDJSON frame in bytes (64 MiB).
///
/// Exceeding this returns `ProtoError::FrameTooLarge`. The caller closes the
/// connection rather than attempting to resync — the byte stream is no longer
/// trustworthy after an oversize frame.
pub const MAX_FRAME_BYTES: usize = 64 << 20;

/// Read a single NDJSON frame from `reader` and deserialise it into `T`.
///
/// Returns `Ok(None)` if the peer closed the connection cleanly between
/// frames (zero bytes available before the first byte). Returns
/// `ProtoError::FrameTooLarge` if the frame exceeds `MAX_FRAME_BYTES` without
/// terminating in `\n`. The internal buffer is bounded by the cap and never
/// grows past it.
pub fn read_frame<R: BufRead, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<Option<T>, ProtoError> {
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            // EOF.
            if buf.is_empty() {
                return Ok(None);
            }
            // Trailing line without newline — accept as a final frame.
            return parse(&buf).map(Some);
        }

        if let Some(nl_idx) = chunk.iter().position(|&b| b == b'\n') {
            // Pull through the newline (inclusive). Don't include the '\n' in
            // the slice we hand to serde_json — it's whitespace anyway, but
            // explicit is clearer.
            if buf.len() + nl_idx > MAX_FRAME_BYTES {
                return Err(ProtoError::FrameTooLarge);
            }
            buf.extend_from_slice(&chunk[..nl_idx]);
            reader.consume(nl_idx + 1);
            return parse(&buf).map(Some);
        }

        // No newline yet; absorb the whole chunk and keep reading.
        if buf.len() + chunk.len() > MAX_FRAME_BYTES {
            return Err(ProtoError::FrameTooLarge);
        }
        buf.extend_from_slice(chunk);
        let n = chunk.len();
        reader.consume(n);
    }
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtoError> {
    serde_json::from_slice(bytes).map_err(ProtoError::Decode)
}

/// Serialise `frame` and write it to `writer` followed by `\n`.
///
/// The writer must be unbuffered or the caller must `flush` after the call —
/// callers downstream (NDJSON over a socket) rely on per-frame visibility.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, frame: &T) -> Result<(), ProtoError> {
    let bytes = serde_json::to_vec(frame)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(ProtoError::FrameTooLarge);
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Length-prefixed, type-tagged framing (ADR 0021, v0.4).
//
// Wire layout per frame:
//
//     [uvarint payload_len][1 byte frame_type][payload]
//
// `payload_len` is an unsigned LEB128 varint counting the bytes of
// `payload` only — it does NOT include the type byte. The 64 MiB cap
// (`MAX_FRAME_BYTES`, THREAT_MODEL F-5) is enforced on `payload_len`
// *before* any payload byte is read, so a hostile length can't drive an
// allocation. JSON control frames keep today's shapes; BLOB frames carry
// raw bytes (decoded media) correlated by attachment id from a prior
// JSON frame.
// ---------------------------------------------------------------------------

/// Frame-type tag, the single byte that follows the length varint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// A UTF-8 JSON control frame (request / response / capabilities).
    Json = 0x01,
    /// Raw bytes (decoded media), keyed by attachment id from a prior JSON frame.
    Blob = 0x02,
}

impl FrameType {
    fn from_byte(b: u8) -> Result<Self, ProtoError> {
        match b {
            0x01 => Ok(FrameType::Json),
            0x02 => Ok(FrameType::Blob),
            other => Err(ProtoError::MalformedFrame(format!(
                "unknown frame-type byte 0x{other:02x}"
            ))),
        }
    }
}

/// One length-prefixed frame read off the wire: its type tag and its raw
/// payload bytes. JSON payloads are deserialised by the caller with
/// [`decode_json_payload`]; BLOB payloads are used as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    /// The frame-type tag.
    pub frame_type: FrameType,
    /// The payload bytes (exactly `payload_len` of them; no type byte, no
    /// trailing delimiter).
    pub payload: Vec<u8>,
}

/// Maximum bytes a `payload_len` varint may occupy. `MAX_FRAME_BYTES`
/// (64 MiB) fits in 27 bits, i.e. 4 LEB128 groups; cap the read at 5 so a
/// non-terminating varint can't spin forever.
const MAX_VARINT_BYTES: usize = 5;

/// Read one length-prefixed, type-tagged frame.
///
/// Returns `Ok(None)` if the peer closed the connection cleanly between
/// frames (EOF before the first length byte). Returns
/// `ProtoError::FrameTooLarge` if `payload_len` exceeds `MAX_FRAME_BYTES`
/// (checked before reading payload), and `ProtoError::MalformedFrame` for
/// an unknown type byte, a non-terminating length varint, or EOF
/// mid-frame.
pub fn read_lp_frame<R: BufRead>(reader: &mut R) -> Result<Option<RawFrame>, ProtoError> {
    // 1. payload_len (LEB128 varint). EOF before the first byte is a clean
    //    between-frames close.
    let payload_len = match read_uvarint(reader)? {
        Some(n) => n,
        None => return Ok(None),
    };
    if payload_len > MAX_FRAME_BYTES as u64 {
        return Err(ProtoError::FrameTooLarge);
    }
    let payload_len = payload_len as usize;

    // 2. frame_type (1 byte). EOF here is mid-frame — malformed.
    let mut type_byte = [0u8; 1];
    read_exact_eofaware(reader, &mut type_byte, "frame-type byte")?;
    let frame_type = FrameType::from_byte(type_byte[0])?;

    // 3. payload (exactly payload_len bytes).
    let mut payload = vec![0u8; payload_len];
    read_exact_eofaware(reader, &mut payload, "frame payload")?;

    Ok(Some(RawFrame {
        frame_type,
        payload,
    }))
}

/// Deserialise a JSON frame's payload into `T`. Use on a [`RawFrame`] whose
/// `frame_type` is [`FrameType::Json`].
pub fn decode_json_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtoError> {
    serde_json::from_slice(payload).map_err(ProtoError::Decode)
}

/// Serialise `frame` as JSON and write it as a length-prefixed JSON frame.
///
/// The writer must be unbuffered or the caller must `flush` afterward —
/// consumers rely on per-frame visibility.
pub fn write_lp_json<W: Write, T: Serialize>(writer: &mut W, frame: &T) -> Result<(), ProtoError> {
    let bytes = serde_json::to_vec(frame)?;
    write_lp_payload(writer, FrameType::Json, &bytes)
}

/// Write `bytes` as a length-prefixed BLOB frame (raw, no encoding).
pub fn write_lp_blob<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<(), ProtoError> {
    write_lp_payload(writer, FrameType::Blob, bytes)
}

fn write_lp_payload<W: Write>(
    writer: &mut W,
    frame_type: FrameType,
    payload: &[u8],
) -> Result<(), ProtoError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ProtoError::FrameTooLarge);
    }
    let mut prefix = [0u8; MAX_VARINT_BYTES];
    let n = write_uvarint(payload.len() as u64, &mut prefix);
    writer.write_all(&prefix[..n])?;
    writer.write_all(&[frame_type as u8])?;
    writer.write_all(payload)?;
    Ok(())
}

/// Read an unsigned LEB128 varint. `Ok(None)` on clean EOF before the
/// first byte; `MalformedFrame` if it doesn't terminate within
/// `MAX_VARINT_BYTES` or EOFs mid-varint.
fn read_uvarint<R: BufRead>(reader: &mut R) -> Result<Option<u64>, ProtoError> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let mut byte = [0u8; 1];
        let got = read_one(reader, &mut byte)?;
        if got == 0 {
            if i == 0 {
                return Ok(None); // clean between-frames EOF
            }
            return Err(ProtoError::MalformedFrame(
                "stream ended mid-length-varint".into(),
            ));
        }
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(Some(value));
        }
        shift += 7;
    }
    Err(ProtoError::MalformedFrame(format!(
        "length varint exceeded {MAX_VARINT_BYTES} bytes"
    )))
}

/// Encode `value` as LEB128 into `out`, returning the number of bytes written.
fn write_uvarint(mut value: u64, out: &mut [u8; MAX_VARINT_BYTES]) -> usize {
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

/// Read exactly `buf.len()` bytes; treat short reads (EOF) as a malformed
/// mid-frame truncation rather than a silent partial.
fn read_exact_eofaware<R: BufRead>(
    reader: &mut R,
    buf: &mut [u8],
    what: &str,
) -> Result<(), ProtoError> {
    match reader.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(ProtoError::MalformedFrame(
            format!("stream ended mid-frame reading {what}"),
        )),
        Err(e) => Err(ProtoError::Io(e)),
    }
}

/// Read a single byte; returns the count read (0 = EOF).
fn read_one<R: BufRead>(reader: &mut R, buf: &mut [u8; 1]) -> Result<usize, ProtoError> {
    loop {
        match reader.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ProtoError::Io(e)),
        }
    }
}

#[cfg(test)]
mod lp_tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::io::Cursor;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Ctrl {
        id: String,
        n: u32,
    }

    #[test]
    fn varint_round_trips_across_boundaries() {
        for v in [0u64, 1, 127, 128, 16383, 16384, (MAX_FRAME_BYTES as u64)] {
            let mut buf = [0u8; MAX_VARINT_BYTES];
            let n = write_uvarint(v, &mut buf);
            let mut cur = Cursor::new(&buf[..n]);
            let got = read_uvarint(&mut cur).unwrap().unwrap();
            assert_eq!(got, v, "varint round-trip for {v}");
        }
    }

    #[test]
    fn json_frame_round_trips() {
        let mut wire = Vec::new();
        write_lp_json(
            &mut wire,
            &Ctrl {
                id: "x".into(),
                n: 7,
            },
        )
        .unwrap();

        let mut cur = Cursor::new(wire);
        let frame = read_lp_frame(&mut cur).unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::Json);
        let ctrl: Ctrl = decode_json_payload(&frame.payload).unwrap();
        assert_eq!(
            ctrl,
            Ctrl {
                id: "x".into(),
                n: 7
            }
        );
        // Clean EOF after the frame.
        assert!(read_lp_frame(&mut cur).unwrap().is_none());
    }

    #[test]
    fn blob_frame_round_trips_including_newline_bytes() {
        // Raw bytes that include 0x0A — newline framing could never carry
        // these; length-prefix can.
        let raw: Vec<u8> = (0u16..=300).map(|b| (b % 256) as u8).collect();
        let mut wire = Vec::new();
        write_lp_blob(&mut wire, &raw).unwrap();

        let mut cur = Cursor::new(wire);
        let frame = read_lp_frame(&mut cur).unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::Blob);
        assert_eq!(frame.payload, raw);
    }

    #[test]
    fn interleaved_json_then_blob() {
        let mut wire = Vec::new();
        write_lp_json(
            &mut wire,
            &Ctrl {
                id: "req".into(),
                n: 1,
            },
        )
        .unwrap();
        write_lp_blob(&mut wire, &[0xDE, 0xAD, 0x0A, 0xBE, 0xEF]).unwrap();

        let mut cur = Cursor::new(wire);
        let f1 = read_lp_frame(&mut cur).unwrap().unwrap();
        assert_eq!(f1.frame_type, FrameType::Json);
        let f2 = read_lp_frame(&mut cur).unwrap().unwrap();
        assert_eq!(f2.frame_type, FrameType::Blob);
        assert_eq!(f2.payload, vec![0xDE, 0xAD, 0x0A, 0xBE, 0xEF]);
        assert!(read_lp_frame(&mut cur).unwrap().is_none());
    }

    #[test]
    fn oversize_length_rejected_before_payload() {
        // Hand-craft a length prefix just over the cap; no payload follows.
        let mut wire = [0u8; MAX_VARINT_BYTES];
        let n = write_uvarint(MAX_FRAME_BYTES as u64 + 1, &mut wire);
        let mut cur = Cursor::new(&wire[..n]);
        match read_lp_frame(&mut cur) {
            Err(ProtoError::FrameTooLarge) => {}
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn unknown_type_byte_is_malformed() {
        let mut wire = Vec::new();
        let mut prefix = [0u8; MAX_VARINT_BYTES];
        let n = write_uvarint(0, &mut prefix); // empty payload
        wire.extend_from_slice(&prefix[..n]);
        wire.push(0x09); // bogus type
        let mut cur = Cursor::new(wire);
        match read_lp_frame(&mut cur) {
            Err(ProtoError::MalformedFrame(_)) => {}
            other => panic!("expected MalformedFrame, got {other:?}"),
        }
    }

    #[test]
    fn truncated_payload_is_malformed() {
        // Claim 10 bytes, provide 3.
        let mut wire = Vec::new();
        let mut prefix = [0u8; MAX_VARINT_BYTES];
        let n = write_uvarint(10, &mut prefix);
        wire.extend_from_slice(&prefix[..n]);
        wire.push(FrameType::Blob as u8);
        wire.extend_from_slice(&[1, 2, 3]);
        let mut cur = Cursor::new(wire);
        match read_lp_frame(&mut cur) {
            Err(ProtoError::MalformedFrame(_)) => {}
            other => panic!("expected MalformedFrame, got {other:?}"),
        }
    }
}
