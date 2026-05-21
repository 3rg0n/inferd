//! Embed request envelope and validation.
//!
//! Per ADR 0017 §"Embed request". Required fields: `id`, `input`
//! (non-empty, each entry non-empty). Optional: `dimensions` (Matryoshka
//! truncation length, validated against the model's supported set at
//! the backend layer), `task` (task-prefix hint).

use crate::error::ProtoError;
use serde::{Deserialize, Serialize};

/// Task-prefix hint for embedding models trained with task-aware
/// prefixes (e.g. EmbeddingGemma). Backends that don't recognise the
/// task ignore the field; the daemon applies the engine-specific
/// prefix on behalf of the consumer per ADR 0013.
///
/// Forward-compatibility: unknown task variants land in `Other` so
/// older daemons / clients tolerate task hints added in later v0.x
/// revisions without rejecting at parse time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedTask {
    /// Encoding a query that will be matched against indexed documents.
    RetrievalQuery,
    /// Encoding a document that will be indexed for retrieval.
    RetrievalDocument,
    /// Encoding text for similarity scoring (symmetric).
    Similarity,
    /// Encoding text whose label will be predicted.
    Classification,
    /// Encoding text for unsupervised clustering.
    Clustering,
    /// Encoding for question-answering pair scoring.
    QuestionAnswering,
    /// Encoding text for fact-verification scoring.
    FactVerification,
    /// Encoding code for code-search retrieval.
    CodeRetrievalQuery,
    /// Forward-compatible escape hatch — unknown task strings deserialise here.
    #[serde(other)]
    Other,
}

/// The embed request envelope sent by clients.
///
/// `Default` is intentionally available for `..Default::default()`
/// shorthand; callers must populate `id` and `input` before sending.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EmbedRequest {
    /// Caller-assigned correlation id; echoed on the response frame.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,

    /// One or more input strings to embed. Each is encoded
    /// independently; the response's `embeddings[i]` corresponds to
    /// `input[i]`.
    pub input: Vec<String>,

    /// Matryoshka truncation length. EmbeddingGemma supports
    /// `768 | 512 | 256 | 128`; backends validate against their own
    /// supported set and emit `invalid_request` if the value is
    /// rejected. Omitted means "model default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,

    /// Task-prefix hint. Backends that don't apply task prefixes
    /// ignore this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<EmbedTask>,
}

/// `EmbedRequest` with semantic validation completed.
///
/// `dimensions` validation against the active backend's supported set
/// happens at the backend layer (different models support different
/// MRL widths), not here.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedResolved {
    /// Caller-assigned correlation id.
    pub id: String,
    /// Validated input strings.
    pub input: Vec<String>,
    /// Truncation length, if set.
    pub dimensions: Option<u32>,
    /// Task hint, if set.
    pub task: Option<EmbedTask>,
}

impl EmbedRequest {
    /// Validate the request envelope. Rejects empty `input` and
    /// empty inner strings. Does NOT validate `dimensions` against any
    /// model-specific supported set — backends do that.
    pub fn resolve(self) -> Result<EmbedResolved, ProtoError> {
        if self.input.is_empty() {
            return Err(ProtoError::InvalidRequest("input must not be empty".into()));
        }
        for (i, s) in self.input.iter().enumerate() {
            if s.is_empty() {
                return Err(ProtoError::InvalidRequest(format!(
                    "input[{i}] must not be empty"
                )));
            }
        }
        if matches!(self.task, Some(EmbedTask::Other)) {
            return Err(ProtoError::InvalidRequest(
                "task uses an unknown variant".into(),
            ));
        }
        Ok(EmbedResolved {
            id: self.id,
            input: self.input,
            dimensions: self.dimensions,
            task: self.task,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_input() {
        let req = EmbedRequest {
            id: "r1".into(),
            input: vec![],
            ..Default::default()
        };
        let err = req.resolve().unwrap_err();
        assert!(matches!(err, ProtoError::InvalidRequest(_)));
    }

    #[test]
    fn rejects_empty_inner_string() {
        let req = EmbedRequest {
            id: "r1".into(),
            input: vec!["hello".into(), String::new()],
            ..Default::default()
        };
        let err = req.resolve().unwrap_err();
        assert!(matches!(err, ProtoError::InvalidRequest(_)));
    }

    #[test]
    fn accepts_minimal_request() {
        let req = EmbedRequest {
            id: "r1".into(),
            input: vec!["hello".into()],
            ..Default::default()
        };
        let resolved = req.resolve().unwrap();
        assert_eq!(resolved.input.len(), 1);
        assert!(resolved.dimensions.is_none());
        assert!(resolved.task.is_none());
    }

    #[test]
    fn parses_full_request_json() {
        let s = r#"{
            "id": "r1",
            "input": ["a", "b"],
            "dimensions": 256,
            "task": "retrieval_document"
        }"#;
        let req: EmbedRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.input.len(), 2);
        assert_eq!(req.dimensions, Some(256));
        assert_eq!(req.task, Some(EmbedTask::RetrievalDocument));
    }

    #[test]
    fn unknown_task_round_trips_as_other() {
        let s = r#"{"id":"r1","input":["x"],"task":"some_future_task"}"#;
        let req: EmbedRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.task, Some(EmbedTask::Other));
        // resolve rejects Other so the daemon doesn't silently apply
        // a task it doesn't know.
        assert!(req.resolve().is_err());
    }

    #[test]
    fn skips_serializing_optional_fields_when_unset() {
        let req = EmbedRequest {
            id: "r1".into(),
            input: vec!["hello".into()],
            dimensions: None,
            task: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains("dimensions"));
        assert!(!s.contains("task"));
    }
}
