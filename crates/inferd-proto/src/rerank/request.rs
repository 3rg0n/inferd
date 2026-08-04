//! Rerank request envelope and validation.
//!
//! Per ADR 0027 §"Rerank request". Required: `id`, `query` (non-empty),
//! `documents` (non-empty, each entry non-empty). Optional: `top_n`
//! (truncate the returned ordering).

use crate::error::ProtoError;
use serde::{Deserialize, Serialize};

/// Maximum documents a single rerank request may carry.
///
/// Rerank is the one surface whose cost is `O(documents)` **forward
/// passes** — unlike embed, where a batch is one pass. The 64 MiB frame
/// cap ([`crate::MAX_FRAME_BYTES`], THREAT_MODEL F-5) bounds *bytes*,
/// not *work*: a single in-cap frame of short documents describes on the
/// order of half a million query/document pairs, each a full model
/// evaluation, all of them holding the shared admission permit. That is
/// the same amplification class as THREAT_MODEL F-1, where one cheap
/// request frame entitled the sender to unbounded reads.
///
/// 256 is well above any real reranking stage — retrieval typically
/// hands the reranker the top 20–100 candidates — and rejecting at parse
/// costs nothing, where discovering the problem at document 400,000 costs
/// a wedged generation slot.
pub const MAX_RERANK_DOCUMENTS: usize = 256;

/// Maximum total text bytes a single rerank request may carry, summed
/// across `query` and every entry of `documents`.
///
/// The companion to [`MAX_RERANK_DOCUMENTS`]: the count cap alone still
/// permits 256 documents of 64 MiB each in aggregate up to the frame cap,
/// and encoding cost scales with tokens, not with document count.
pub const MAX_RERANK_TOTAL_BYTES: usize = 8 * 1024 * 1024;

/// The rerank request envelope sent by clients.
///
/// `Default` is available for `..Default::default()` shorthand; callers
/// must populate `id`, `query`, and `documents` before sending.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RerankRequest {
    /// Caller-assigned correlation id; echoed on the response frame.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,

    /// The search query every document is scored against.
    pub query: String,

    /// Candidate documents to score. The response's `results[].index`
    /// refers back into this array.
    pub documents: Vec<String>,

    /// Return only the `n` highest-scoring results. Omitted returns all
    /// of them. `0` is rejected — an empty result set is never what a
    /// caller wants, and returning one silently would be
    /// indistinguishable from a backend that scored nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_n: Option<u32>,
}

/// `RerankRequest` with semantic validation completed.
///
/// Constructible only via [`RerankRequest::resolve`] outside this
/// crate's tests, so holding one is proof the bounds and non-emptiness
/// checks ran.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankResolved {
    /// Caller-assigned correlation id.
    pub id: String,
    /// Validated query string.
    pub query: String,
    /// Validated candidate documents.
    pub documents: Vec<String>,
    /// Truncation length, if set. Guaranteed non-zero.
    pub top_n: Option<u32>,
}

impl RerankRequest {
    /// Validate the request envelope.
    ///
    /// Rejects an empty `query`, an empty `documents` array, any empty
    /// document, `top_n: 0`, and anything over
    /// [`MAX_RERANK_DOCUMENTS`] / [`MAX_RERANK_TOTAL_BYTES`].
    ///
    /// A `top_n` larger than `documents.len()` is *not* an error — it
    /// means "all of them", the same as omitting it. Callers paginating
    /// a shrinking candidate set should not have to clamp.
    pub fn resolve(self) -> Result<RerankResolved, ProtoError> {
        if self.query.is_empty() {
            return Err(ProtoError::InvalidRequest("query must not be empty".into()));
        }
        if self.documents.is_empty() {
            return Err(ProtoError::InvalidRequest(
                "documents must not be empty".into(),
            ));
        }
        if self.documents.len() > MAX_RERANK_DOCUMENTS {
            return Err(ProtoError::InvalidRequest(format!(
                "request carries {} documents; at most {MAX_RERANK_DOCUMENTS} allowed",
                self.documents.len()
            )));
        }
        for (i, d) in self.documents.iter().enumerate() {
            if d.is_empty() {
                return Err(ProtoError::InvalidRequest(format!(
                    "documents[{i}] must not be empty"
                )));
            }
        }
        // Sum with `usize` saturation rather than `+`: the inputs are
        // already bounded by the frame cap, so this cannot overflow in
        // practice, but a panic on a wire-derived value is never the
        // right failure mode.
        let total: usize = self
            .documents
            .iter()
            .map(|d| d.len())
            .fold(self.query.len(), |acc, n| acc.saturating_add(n));
        if total > MAX_RERANK_TOTAL_BYTES {
            return Err(ProtoError::InvalidRequest(format!(
                "request carries {total} bytes of query + documents; at most \
                 {MAX_RERANK_TOTAL_BYTES} allowed"
            )));
        }
        if self.top_n == Some(0) {
            return Err(ProtoError::InvalidRequest(
                "top_n must be greater than zero when set".into(),
            ));
        }
        Ok(RerankResolved {
            id: self.id,
            query: self.query,
            documents: self.documents,
            top_n: self.top_n,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(query: &str, docs: &[&str]) -> RerankRequest {
        RerankRequest {
            id: "r1".into(),
            query: query.into(),
            documents: docs.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn accepts_minimal_request() {
        let resolved = req("q", &["d"]).resolve().unwrap();
        assert_eq!(resolved.query, "q");
        assert_eq!(resolved.documents.len(), 1);
        assert!(resolved.top_n.is_none());
    }

    #[test]
    fn rejects_empty_query() {
        let err = req("", &["d"]).resolve().unwrap_err();
        assert!(matches!(err, ProtoError::InvalidRequest(_)));
        assert!(err.to_string().contains("query must not be empty"));
    }

    #[test]
    fn rejects_empty_documents() {
        let err = req("q", &[]).resolve().unwrap_err();
        assert!(err.to_string().contains("documents must not be empty"));
    }

    #[test]
    fn rejects_empty_inner_document() {
        let err = req("q", &["a", ""]).resolve().unwrap_err();
        assert!(err.to_string().contains("documents[1]"));
    }

    #[test]
    fn rejects_zero_top_n() {
        let mut r = req("q", &["d"]);
        r.top_n = Some(0);
        let err = r.resolve().unwrap_err();
        assert!(err.to_string().contains("top_n"));
    }

    #[test]
    fn accepts_top_n_larger_than_documents() {
        // "more than you have" means "all of them" — a caller whose
        // candidate set shrank should not have to clamp.
        let mut r = req("q", &["a", "b"]);
        r.top_n = Some(50);
        assert_eq!(r.resolve().unwrap().top_n, Some(50));
    }

    #[test]
    fn rejects_document_count_over_cap() {
        let docs: Vec<String> = (0..MAX_RERANK_DOCUMENTS + 1)
            .map(|i| i.to_string())
            .collect();
        let r = RerankRequest {
            id: "r1".into(),
            query: "q".into(),
            documents: docs,
            top_n: None,
        };
        let err = r.resolve().unwrap_err();
        assert!(
            err.to_string().contains("at most 256 allowed"),
            "got: {err}"
        );
    }

    #[test]
    fn accepts_document_count_at_cap() {
        let docs: Vec<String> = (0..MAX_RERANK_DOCUMENTS).map(|i| i.to_string()).collect();
        let r = RerankRequest {
            id: "r1".into(),
            query: "q".into(),
            documents: docs,
            top_n: None,
        };
        assert_eq!(r.resolve().unwrap().documents.len(), MAX_RERANK_DOCUMENTS);
    }

    #[test]
    fn rejects_total_bytes_over_cap() {
        // Two documents inside the count cap but over the byte budget:
        // the count cap alone does not bound encoding cost.
        let big = "x".repeat(MAX_RERANK_TOTAL_BYTES / 2 + 16);
        let r = RerankRequest {
            id: "r1".into(),
            query: "q".into(),
            documents: vec![big.clone(), big],
            top_n: None,
        };
        let err = r.resolve().unwrap_err();
        assert!(
            err.to_string().contains("bytes of query + documents"),
            "got: {err}"
        );
    }

    #[test]
    fn parses_full_request_json() {
        let s = r#"{
            "id": "r1",
            "query": "how do I disable the GPU?",
            "documents": ["set n_gpu_layers to 0", "install CUDA"],
            "top_n": 1
        }"#;
        let r: RerankRequest = serde_json::from_str(s).unwrap();
        assert_eq!(r.documents.len(), 2);
        assert_eq!(r.top_n, Some(1));
        assert_eq!(r.resolve().unwrap().top_n, Some(1));
    }

    #[test]
    fn unknown_fields_are_ignored_on_parse() {
        // The additive-change door stays open: an older daemon must
        // tolerate fields a newer client sends.
        let s = r#"{"id":"r1","query":"q","documents":["d"],"future_field":true}"#;
        let r: RerankRequest = serde_json::from_str(s).unwrap();
        assert_eq!(r.query, "q");
    }

    #[test]
    fn skips_serializing_optional_fields_when_unset() {
        let r = req("q", &["d"]);
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("top_n"));
    }
}
