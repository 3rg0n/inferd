//! Embed response frame schema.
//!
//! Per ADR 0017 §"Embed response". Single terminal frame per request
//! — embeddings are not streamed. Two variants: `Embeddings` (success)
//! and `Error` (failure).

use serde::{Deserialize, Serialize};

/// Token-count usage report carried on `embeddings` frames.
///
/// Embed requests have no output tokens (the output is a vector, not
/// a generation), so only `input_tokens` is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedUsage {
    /// Tokens consumed by the input strings (sum across the batch).
    pub input_tokens: u32,
}

/// Embed-specific error-code taxonomy.
///
/// Superset of v1's `ErrorCode` (kept independent so the v1 enum
/// stays frozen per ADR 0008). The only embed-specific addition is
/// `embed_unsupported`, returned in the belt-and-braces case where a
/// daemon configured with a generation-only backend somehow receives
/// an embed request (the embed socket should not have been bound in
/// that configuration — the error is a fail-safe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedErrorCode {
    /// Admission queue full at submit time.
    QueueFull,
    /// Selected backend errored before or during embedding.
    BackendUnavailable,
    /// Request failed validation (empty input, unsupported dimensions,
    /// unknown task, etc.).
    InvalidRequest,
    /// Frame exceeded the 64 MiB cap.
    FrameTooLarge,
    /// Daemon-side bug or unexpected condition.
    Internal,
    /// The active backend doesn't support embeddings.
    EmbedUnsupported,
}

/// One frame on the embed response stream.
///
/// Always terminal — there are exactly two outcomes, success
/// (`Embeddings`) or failure (`Error`). The connection stays open for
/// the next request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbedResponse {
    /// Successful embedding result.
    Embeddings {
        /// Request id.
        id: String,
        /// One vector per input string, in the same order as the
        /// request's `input`. Inner vectors all share the same
        /// `dimensions` length.
        embeddings: Vec<Vec<f32>>,
        /// Actual length of each inner vector after any MRL truncation.
        dimensions: u32,
        /// Backend-reported model name (e.g. `"embeddinggemma-300m"`).
        model: String,
        /// Token-count usage.
        usage: EmbedUsage,
        /// `Backend::name()` of the adapter that served this request.
        ///
        /// Diagnostic only — apps must not branch on this (ADR 0007).
        backend: String,
    },
    /// Failure terminal frame.
    Error {
        /// Request id.
        id: String,
        /// Machine-readable classification.
        code: EmbedErrorCode,
        /// Human-readable description.
        message: String,
    },
}

impl EmbedResponse {
    /// Correlation id of the frame regardless of variant.
    pub fn id(&self) -> &str {
        match self {
            EmbedResponse::Embeddings { id, .. } | EmbedResponse::Error { id, .. } => id,
        }
    }

    /// `true` if this frame represents a successful embedding result.
    pub fn is_ok(&self) -> bool {
        matches!(self, EmbedResponse::Embeddings { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddings_variant_round_trips() {
        let resp = EmbedResponse::Embeddings {
            id: "r1".into(),
            embeddings: vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]],
            dimensions: 3,
            model: "embeddinggemma-300m".into(),
            usage: EmbedUsage { input_tokens: 12 },
            backend: "llamacpp".into(),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: EmbedResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(resp, back);
        assert!(resp.is_ok());
        assert_eq!(resp.id(), "r1");
    }

    #[test]
    fn error_variant_round_trips() {
        let resp = EmbedResponse::Error {
            id: "r1".into(),
            code: EmbedErrorCode::InvalidRequest,
            message: "dimensions must be one of [128, 256, 512, 768]".into(),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: EmbedResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(resp, back);
        assert!(!resp.is_ok());
    }

    #[test]
    fn embeddings_serializes_with_type_tag() {
        let resp = EmbedResponse::Embeddings {
            id: "r1".into(),
            embeddings: vec![vec![0.1]],
            dimensions: 1,
            model: "m".into(),
            usage: EmbedUsage { input_tokens: 1 },
            backend: "llamacpp".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "embeddings");
        assert_eq!(v["dimensions"], 1);
    }

    #[test]
    fn error_code_serializes_snake_case() {
        let s = serde_json::to_string(&EmbedErrorCode::EmbedUnsupported).unwrap();
        assert_eq!(s, "\"embed_unsupported\"");
    }
}
