//! TCP API-key authentication. THREAT_MODEL F-8.
//!
//! UDS and named-pipe transports use kernel-attested peer credentials
//! (F-7). Loopback TCP cannot — anything on the host can bind 127.0.0.1
//! — so when the operator enables `--tcp` they may also set
//! `--api-key`. Clients that connect over TCP must then send an auth
//! frame as the first NDJSON object on the connection:
//!
//! ```json
//! {"type":"auth","key":"<the configured value>"}
//! ```
//!
//! The daemon parses this, compares the key with
//! `subtle::ConstantTimeEq`, and then proceeds to normal request
//! handling. A missing or wrong key closes the connection with no
//! error frame written (we don't want to confirm the endpoint exists
//! to anonymous probers).

use serde::Deserialize;
use subtle::ConstantTimeEq;

/// Auth-frame shape. `type` discriminator + `key` payload.
#[derive(Debug, Deserialize)]
pub struct AuthFrame {
    /// Must be the literal `"auth"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Pre-shared key value.
    pub key: String,
}

impl AuthFrame {
    /// Parse a single line of NDJSON into an `AuthFrame`. Returns
    /// `None` if the line decodes but `type != "auth"` so the caller
    /// can distinguish "well-formed but wrong" from "garbage."
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let frame: AuthFrame = serde_json::from_slice(bytes).ok()?;
        if frame.kind != "auth" {
            return None;
        }
        Some(frame)
    }
}

/// Constant-time comparison of two API-key strings. Identical to
/// `==` semantically but doesn't leak how many leading bytes match.
pub fn key_matches(presented: &str, expected: &str) -> bool {
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_frame() {
        let frame = AuthFrame::from_json(br#"{"type":"auth","key":"secret"}"#).unwrap();
        assert_eq!(frame.key, "secret");
    }

    #[test]
    fn rejects_non_auth_type() {
        let frame = AuthFrame::from_json(br#"{"type":"request","messages":[]}"#);
        assert!(frame.is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(AuthFrame::from_json(b"not json").is_none());
    }

    #[test]
    fn key_matches_equal() {
        assert!(key_matches("hello", "hello"));
    }

    #[test]
    fn key_matches_different() {
        assert!(!key_matches("hello", "helLo"));
        assert!(!key_matches("short", "longer-string"));
    }
}
