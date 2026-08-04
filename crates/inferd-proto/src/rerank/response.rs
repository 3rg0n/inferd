//! Rerank response frame schema.
//!
//! Per ADR 0027 §"Rerank response". Single terminal frame per request —
//! a rerank result is a complete ordering, so there is nothing to
//! stream. Two variants: `Rerank` (success) and `Error` (failure).

use serde::{Deserialize, Serialize};

/// Token-count usage report carried on `rerank` frames.
///
/// Rerank has no output tokens (the output is a set of scores, not a
/// generation), so only `input_tokens` is reported — summed across every
/// query/document pair evaluated, which is why it can be much larger
/// than the request's byte length would suggest: the query is
/// re-encoded once per document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerankUsage {
    /// Tokens consumed across all query/document pairs.
    pub input_tokens: u32,
}

/// One scored document.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RerankResult {
    /// Position of this document in the *request's* `documents` array.
    ///
    /// Indices rather than echoed text: the caller already holds the
    /// documents, and echoing them would multiply an 8 MiB request into
    /// an 8 MiB response for no added information.
    pub index: u32,
    /// Relevance score. Higher is more relevant.
    ///
    /// **Not normalised and not comparable across models.** Some
    /// rerankers emit raw logits, others sigmoid probabilities; the
    /// daemon reports what the classification head produced rather than
    /// squashing it into a fake 0..1 range that would make incomparable
    /// numbers look comparable. Treat it as ordinal within one response.
    pub score: f32,
}

/// Rerank-specific error-code taxonomy.
///
/// Mirrors [`crate::embed::EmbedErrorCode`] but kept independent so each
/// surface's enum can be frozen on its own schedule (the same reason
/// embed's is independent of v1's). The rerank-specific addition is
/// `rerank_unsupported`, a fail-safe for a rerank request reaching a
/// daemon whose backend cannot serve one — the rerank socket should not
/// have been bound in that configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankErrorCode {
    /// Admission queue full at submit time.
    QueueFull,
    /// Selected backend errored before or during scoring.
    BackendUnavailable,
    /// Request failed validation (empty query/documents, over the
    /// document or byte cap, `top_n: 0`).
    InvalidRequest,
    /// Frame exceeded the 64 MiB cap.
    FrameTooLarge,
    /// Daemon-side bug or unexpected condition.
    Internal,
    /// The active backend doesn't support reranking.
    RerankUnsupported,
}

/// One frame on the rerank response stream.
///
/// Always terminal — success (`Rerank`) or failure (`Error`). The
/// connection stays open for the next request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RerankResponse {
    /// Successful rerank result.
    Rerank {
        /// Request id.
        id: String,
        /// Scored documents, sorted by `score` **descending**.
        ///
        /// Sorting is the daemon's job: score scales are
        /// model-specific, so descending-by-score is the only ordering
        /// every model agrees on, and leaving it to consumers invites
        /// each one to re-derive it. Truncated to the request's `top_n`
        /// when set.
        results: Vec<RerankResult>,
        /// Backend-reported model name (e.g. `"bge-reranker-v2-m3"`).
        model: String,
        /// Token-count usage.
        usage: RerankUsage,
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
        code: RerankErrorCode,
        /// Human-readable description.
        message: String,
    },
}

impl RerankResponse {
    /// Correlation id of the frame regardless of variant.
    pub fn id(&self) -> &str {
        match self {
            RerankResponse::Rerank { id, .. } | RerankResponse::Error { id, .. } => id,
        }
    }

    /// `true` if this frame represents a successful rerank result.
    pub fn is_ok(&self) -> bool {
        matches!(self, RerankResponse::Rerank { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_variant_round_trips() {
        let resp = RerankResponse::Rerank {
            id: "r1".into(),
            results: vec![
                RerankResult {
                    index: 3,
                    score: 0.98,
                },
                RerankResult {
                    index: 0,
                    score: -1.5,
                },
            ],
            model: "bge-reranker-v2-m3".into(),
            usage: RerankUsage { input_tokens: 42 },
            backend: "llamacpp".into(),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: RerankResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(resp, back);
        assert!(resp.is_ok());
        assert_eq!(resp.id(), "r1");
    }

    #[test]
    fn error_variant_round_trips() {
        let resp = RerankResponse::Error {
            id: "r1".into(),
            code: RerankErrorCode::RerankUnsupported,
            message: "backend does not support reranking".into(),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: RerankResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(resp, back);
        assert!(!resp.is_ok());
    }

    #[test]
    fn rerank_serializes_with_type_tag() {
        let resp = RerankResponse::Rerank {
            id: "r1".into(),
            results: vec![RerankResult {
                index: 0,
                score: 1.0,
            }],
            model: "m".into(),
            usage: RerankUsage { input_tokens: 1 },
            backend: "llamacpp".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "rerank");
        assert_eq!(v["results"][0]["index"], 0);
    }

    #[test]
    fn error_code_serializes_snake_case() {
        let s = serde_json::to_string(&RerankErrorCode::RerankUnsupported).unwrap();
        assert_eq!(s, "\"rerank_unsupported\"");
    }

    /// Negative scores are ordinary — several rerankers emit raw logits.
    /// A response type that only round-tripped positive scores would
    /// break on the first real model.
    #[test]
    fn negative_scores_round_trip() {
        let r = RerankResult {
            index: 7,
            score: -4.25,
        };
        let back: RerankResult = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.score, -4.25);
    }
}
