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
