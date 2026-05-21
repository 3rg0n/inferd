//! AWS event-stream framing parser.
//!
//! Bedrock's `InvokeModelWithResponseStream` returns binary frames in
//! the `application/vnd.amazon.eventstream` format — *not* SSE. Each
//! frame is length-prefixed and CRC-protected:
//!
//! ```text
//!   +---------------+---------------+
//!   | total_len     | headers_len   |   8 bytes (big-endian u32 each)
//!   +---------------+---------------+
//!   | prelude_crc                   |   4 bytes (CRC32 of the first 8)
//!   +-------------------------------+
//!   | headers (key-value, typed)    |   headers_len bytes
//!   +-------------------------------+
//!   | payload                       |   total_len - headers_len - 16
//!   +-------------------------------+
//!   | message_crc                   |   4 bytes (CRC32 of frame so far)
//!   +-------------------------------+
//! ```
//!
//! Headers are AWS' typed-header format (1-byte name length, name
//! bytes, 1-byte value type, type-specific value). For Bedrock's
//! response stream we only need a few:
//!
//! - `:event-type` — `"chunk"` (data) or `"error"` (terminal error).
//! - `:content-type` — `"application/json"` for chunk payloads.
//! - `:message-type` — `"event"` or `"exception"`.
//!
//! The payload of a `chunk` event is JSON `{"bytes": "<base64>"}`
//! whose decoded value is one inner Anthropic SSE-shaped event.
//!
//! v0.2.0 scope: parse exactly the subset Bedrock emits for invoke-
//! with-response-stream + Anthropic-on-Bedrock. We don't validate
//! CRCs (we trust TLS for transport integrity) and we don't implement
//! the full header type table — only string-typed headers, which is
//! all Bedrock uses for the ones we read.

use bytes::{Buf, BytesMut};

/// One parsed event-stream frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Frame {
    /// `:event-type` header value, e.g. `"chunk"`, `"error"`.
    pub event_type: String,
    /// `:exception-type` header value when present (set on
    /// `:message-type: exception` frames).
    pub exception_type: Option<String>,
    /// Frame payload bytes. For `chunk` events this is JSON
    /// `{"bytes": "<base64>"}`; for exceptions, JSON `{"message": "..."}`.
    pub payload: Vec<u8>,
}

/// Errors parsing an event-stream frame.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum FrameError {
    /// Frame's prelude announces a length that doesn't fit our own
    /// safety cap. Bedrock's frames are bounded but we cap to avoid
    /// runaway allocation on a corrupt stream.
    #[error("frame too large: {0} bytes")]
    TooLarge(u32),
    /// Frame announces lengths that are inconsistent with each other.
    #[error("malformed frame: total={total} headers={headers}")]
    Malformed {
        /// Frame's announced total length in bytes.
        total: u32,
        /// Frame's announced headers length in bytes.
        headers: u32,
    },
}

/// Maximum frame length we'll allocate for. Bedrock's frames are far
/// below this in practice; we cap to bound memory on a corrupt stream.
const MAX_FRAME_BYTES: u32 = 1024 * 1024;

/// Streaming event-stream decoder. Push raw bytes via [`feed`]; pull
/// completed frames via [`next_frame`].
#[derive(Debug, Default)]
pub(super) struct EventStreamDecoder {
    buf: BytesMut,
}

impl EventStreamDecoder {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Append upstream bytes to the internal buffer.
    pub(super) fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Try to parse one complete frame off the front of the buffer.
    /// Returns `Ok(None)` when more bytes are needed.
    pub(super) fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.buf.len() < 12 {
            return Ok(None);
        }
        // Peek the prelude without consuming.
        let total_len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        let headers_len = u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);

        if total_len > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(total_len));
        }
        // total = 12 (prelude+prelude_crc) + headers_len + payload + 4 (message_crc)
        // → payload_len = total - 16 - headers_len
        if total_len < 16 || headers_len > total_len.saturating_sub(16) {
            return Err(FrameError::Malformed {
                total: total_len,
                headers: headers_len,
            });
        }
        if (self.buf.len() as u32) < total_len {
            return Ok(None);
        }

        // Consume prelude + prelude_crc.
        self.buf.advance(12);
        let mut headers_remaining = headers_len as usize;
        let payload_len = total_len as usize - 16 - headers_len as usize;

        let mut event_type = String::new();
        let mut exception_type: Option<String> = None;
        let mut message_type: Option<String> = None;

        while headers_remaining > 0 {
            // Header layout: 1-byte name length, name bytes, 1-byte
            // value type, type-specific value bytes.
            if self.buf.is_empty() {
                return Err(FrameError::Malformed {
                    total: total_len,
                    headers: headers_len,
                });
            }
            let name_len = self.buf[0] as usize;
            if 1 + name_len + 1 > headers_remaining || 1 + name_len + 1 > self.buf.len() {
                return Err(FrameError::Malformed {
                    total: total_len,
                    headers: headers_len,
                });
            }
            let name_bytes = self.buf[1..1 + name_len].to_vec();
            let value_type = self.buf[1 + name_len];
            let header_fixed = 1 + name_len + 1;
            self.buf.advance(header_fixed);
            headers_remaining -= header_fixed;

            // value_type 7 = string (2-byte length, then bytes). Other
            // types exist but Bedrock's response headers we care about
            // are all strings; we skip the rest.
            let value_str = if value_type == 7 {
                if self.buf.len() < 2 || headers_remaining < 2 {
                    return Err(FrameError::Malformed {
                        total: total_len,
                        headers: headers_len,
                    });
                }
                let vlen = u16::from_be_bytes([self.buf[0], self.buf[1]]) as usize;
                if 2 + vlen > headers_remaining || 2 + vlen > self.buf.len() {
                    return Err(FrameError::Malformed {
                        total: total_len,
                        headers: headers_len,
                    });
                }
                let v = self.buf[2..2 + vlen].to_vec();
                self.buf.advance(2 + vlen);
                headers_remaining -= 2 + vlen;
                Some(String::from_utf8_lossy(&v).into_owned())
            } else {
                // Skip non-string headers — type-prefixed length per
                // AWS' table. We don't see these on Bedrock's invoke
                // response; if Amazon adds one, we'd want to extend
                // this match. For now we take the safe route: bail.
                return Err(FrameError::Malformed {
                    total: total_len,
                    headers: headers_len,
                });
            };

            let name = String::from_utf8_lossy(&name_bytes);
            match (name.as_ref(), value_str.as_deref()) {
                (":event-type", Some(v)) => event_type = v.to_string(),
                (":exception-type", Some(v)) => exception_type = Some(v.to_string()),
                (":message-type", Some(v)) => message_type = Some(v.to_string()),
                _ => {}
            }
        }

        // Consume payload + message_crc.
        let mut payload = vec![0u8; payload_len];
        self.buf.copy_to_slice(&mut payload);
        self.buf.advance(4); // message_crc

        // Promote `:exception-type` from message-type=exception only;
        // otherwise discard.
        if message_type.as_deref() != Some("exception") {
            exception_type = None;
        }

        Ok(Some(Frame {
            event_type,
            exception_type,
            payload,
        }))
    }
}

/// Inner JSON shape of a `chunk` event payload.
#[derive(Debug, serde::Deserialize)]
pub(super) struct ChunkPayload {
    /// Base64-encoded inner SSE event JSON.
    pub bytes: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one event-stream frame with a `:event-type` header.
    /// Doesn't bother with valid CRCs — the decoder doesn't check them.
    fn build_frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        // Header: name=":event-type", value=string event_type.
        header.push(":event-type".len() as u8);
        header.extend_from_slice(b":event-type");
        header.push(7u8); // string type
        header.extend_from_slice(&(event_type.len() as u16).to_be_bytes());
        header.extend_from_slice(event_type.as_bytes());

        let total = 12 + header.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total);
        frame.extend_from_slice(&(total as u32).to_be_bytes());
        frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0u8; 4]); // prelude_crc (unchecked)
        frame.extend_from_slice(&header);
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&[0u8; 4]); // message_crc (unchecked)
        frame
    }

    fn build_exception_frame(exception_type: &str, payload: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();

        // :event-type
        header.push(":event-type".len() as u8);
        header.extend_from_slice(b":event-type");
        header.push(7u8);
        header.extend_from_slice(&("error".len() as u16).to_be_bytes());
        header.extend_from_slice(b"error");

        // :message-type
        header.push(":message-type".len() as u8);
        header.extend_from_slice(b":message-type");
        header.push(7u8);
        header.extend_from_slice(&("exception".len() as u16).to_be_bytes());
        header.extend_from_slice(b"exception");

        // :exception-type
        header.push(":exception-type".len() as u8);
        header.extend_from_slice(b":exception-type");
        header.push(7u8);
        header.extend_from_slice(&(exception_type.len() as u16).to_be_bytes());
        header.extend_from_slice(exception_type.as_bytes());

        let total = 12 + header.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total);
        frame.extend_from_slice(&(total as u32).to_be_bytes());
        frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0u8; 4]);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&[0u8; 4]);
        frame
    }

    #[test]
    fn parses_one_chunk_frame() {
        let frame_bytes = build_frame("chunk", br#"{"bytes":"aGVsbG8="}"#);
        let mut dec = EventStreamDecoder::new();
        dec.feed(&frame_bytes);
        let f = dec.next_frame().unwrap().unwrap();
        assert_eq!(f.event_type, "chunk");
        assert_eq!(f.payload, br#"{"bytes":"aGVsbG8="}"#);
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn parses_two_back_to_back_frames() {
        let mut buf = build_frame("chunk", br#"{"bytes":"YQ=="}"#);
        buf.extend_from_slice(&build_frame("chunk", br#"{"bytes":"Yg=="}"#));
        let mut dec = EventStreamDecoder::new();
        dec.feed(&buf);
        let a = dec.next_frame().unwrap().unwrap();
        let b = dec.next_frame().unwrap().unwrap();
        assert_eq!(a.event_type, "chunk");
        assert_eq!(b.event_type, "chunk");
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn handles_partial_feed() {
        let frame = build_frame("chunk", br#"{"bytes":"YQ=="}"#);
        let mut dec = EventStreamDecoder::new();
        // Feed in 3-byte chunks.
        for chunk in frame.chunks(3) {
            assert!(dec.next_frame().unwrap().is_none() || dec.next_frame().unwrap().is_some());
            dec.feed(chunk);
        }
        let f = dec.next_frame().unwrap().unwrap();
        assert_eq!(f.event_type, "chunk");
    }

    #[test]
    fn surfaces_exception_type() {
        let frame = build_exception_frame("ThrottlingException", br#"{"message":"slow down"}"#);
        let mut dec = EventStreamDecoder::new();
        dec.feed(&frame);
        let f = dec.next_frame().unwrap().unwrap();
        assert_eq!(f.event_type, "error");
        assert_eq!(f.exception_type.as_deref(), Some("ThrottlingException"));
    }

    #[test]
    fn rejects_overlarge_frame() {
        let mut dec = EventStreamDecoder::new();
        // Announce a 10 MB frame; nothing else needs to be valid.
        let mut prelude = Vec::new();
        prelude.extend_from_slice(&10_000_000u32.to_be_bytes());
        prelude.extend_from_slice(&0u32.to_be_bytes());
        prelude.extend_from_slice(&[0u8; 4]);
        dec.feed(&prelude);
        let err = dec.next_frame().unwrap_err();
        assert!(matches!(err, FrameError::TooLarge(10_000_000)));
    }

    #[test]
    fn parses_chunk_payload_json() {
        let f = build_frame("chunk", br#"{"bytes":"aGVsbG8="}"#);
        let mut dec = EventStreamDecoder::new();
        dec.feed(&f);
        let frame = dec.next_frame().unwrap().unwrap();
        let chunk: ChunkPayload = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(chunk.bytes, "aGVsbG8=");
    }
}
