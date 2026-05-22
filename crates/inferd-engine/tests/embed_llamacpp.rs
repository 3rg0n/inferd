//! Tier 3 integration tests for `LlamaCpp::embed`.
//!
//! Exercises the real FFI path against an EmbeddingGemma 300M GGUF
//! file: dedicated embed context, llama_encode, llama_get_embeddings_seq,
//! all 8 task prefixes, MRL truncation, L2 normalisation, and the GGUF
//! `general.name` model-label lookup.
//!
//! Gated behind the `llamacpp-integration` cargo feature and skips
//! itself with an explanatory message if `INFERD_TEST_EMBED_MODEL_PATH`
//! is unset.
//!
//! To run locally:
//!   cargo test -p inferd-engine \
//!     --features llamacpp-integration \
//!     --test embed_llamacpp \
//!     -- --nocapture
//! with `INFERD_TEST_EMBED_MODEL_PATH=/path/to/embeddinggemma-300m.gguf`.

#![cfg(feature = "llamacpp-integration")]

use inferd_engine::Backend;
use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_proto::embed::{EmbedRequest, EmbedTask};
use std::path::PathBuf;
use std::time::Duration;

fn embed_model_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_EMBED_MODEL_PATH").map(PathBuf::from)
}

fn skipping_msg() {
    eprintln!(
        "[skip] INFERD_TEST_EMBED_MODEL_PATH not set; skipping tier-3 \
         embed integration test. Point it at an EmbeddingGemma 300M GGUF."
    );
}

fn build_backend(path: PathBuf) -> LlamaCpp {
    LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        embed: true,
        embed_n_ctx: 2048,
        ..Default::default()
    })
    .expect("construct LlamaCpp with embed=true")
}

fn req(input: Vec<&str>) -> inferd_proto::embed::EmbedResolved {
    EmbedRequest {
        id: "e1".into(),
        input: input.into_iter().map(String::from).collect(),
        ..Default::default()
    }
    .resolve()
    .expect("resolve embed request")
}

fn req_with(
    input: Vec<&str>,
    dimensions: Option<u32>,
    task: Option<EmbedTask>,
) -> inferd_proto::embed::EmbedResolved {
    EmbedRequest {
        id: "e1".into(),
        input: input.into_iter().map(String::from).collect(),
        dimensions,
        task,
    }
    .resolve()
    .expect("resolve embed request")
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[tokio::test]
async fn advertises_embed_capability() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    assert!(backend.ready());
    let caps = backend.capabilities();
    assert!(caps.embed, "expected capabilities().embed = true");
    assert_eq!(backend.name(), "llamacpp");
}

#[tokio::test]
async fn returns_default_dim_768_unit_norm_vector() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let result = tokio::time::timeout(Duration::from_secs(60), backend.embed(req(vec!["hello"])))
        .await
        .expect("embed timed out")
        .expect("embed succeeded");

    assert_eq!(
        result.embeddings.len(),
        1,
        "expected one vector for one input"
    );
    assert_eq!(
        result.dimensions, 768,
        "EmbeddingGemma 300M default dim is 768"
    );
    assert_eq!(result.embeddings[0].len(), 768);
    let norm = l2_norm(&result.embeddings[0]);
    assert!(
        (norm - 1.0).abs() < 1e-3,
        "expected unit-norm vector, got |v| = {norm}"
    );
    assert!(result.usage.input_tokens > 0);
    // Model label should be the GGUF general.name (or the file stem if
    // that key is absent). Either way it must NOT be the literal
    // "llamacpp" — Backend::name() already exposes that.
    assert_ne!(
        result.model, "llamacpp",
        "model field should be the model identifier, not the backend name"
    );
    assert!(!result.model.is_empty(), "model label must not be empty");
    let lower = result.model.to_lowercase();
    assert!(
        lower.contains("gemma") || lower.contains("embedd") || lower.contains("300m"),
        "model label {:?} should reference the loaded model",
        result.model
    );
}

#[tokio::test]
async fn batches_inputs_in_order() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        backend.embed(req(vec!["hello", "the quick brown fox", "goodbye"])),
    )
    .await
    .expect("embed timed out")
    .expect("embed succeeded");

    assert_eq!(result.embeddings.len(), 3);
    for v in &result.embeddings {
        assert_eq!(v.len(), 768);
        let n = l2_norm(v);
        assert!(
            (n - 1.0).abs() < 1e-3,
            "every batch element must be unit-norm, got {n}"
        );
    }

    // Sanity: distinct inputs produce distinct vectors.
    let c01 = cosine(&result.embeddings[0], &result.embeddings[1]);
    let c02 = cosine(&result.embeddings[0], &result.embeddings[2]);
    assert!(
        c01.abs() < 0.999 && c02.abs() < 0.999,
        "expected distinct vectors, got cos(0,1)={c01} cos(0,2)={c02}"
    );
}

#[tokio::test]
async fn mrl_truncation_to_256_stays_unit_norm() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        backend.embed(req_with(vec!["hello"], Some(256), None)),
    )
    .await
    .expect("embed timed out")
    .expect("embed succeeded");

    assert_eq!(result.dimensions, 256);
    assert_eq!(result.embeddings[0].len(), 256);
    let norm = l2_norm(&result.embeddings[0]);
    assert!(
        (norm - 1.0).abs() < 1e-3,
        "MRL-truncated vector must be re-normalised to unit length, got {norm}"
    );
}

#[tokio::test]
async fn rejects_dimensions_above_n_embd() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let result = backend
        .embed(req_with(vec!["hello"], Some(99_999), None))
        .await;
    assert!(
        matches!(
            result.as_ref().err(),
            Some(inferd_engine::EmbedError::InvalidRequest(_))
        ),
        "expected InvalidRequest, got {:?}",
        result.err()
    );
}

#[tokio::test]
async fn task_prefix_changes_embedding() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let unprefixed = backend
        .embed(req_with(vec!["dragons breathe fire"], None, None))
        .await
        .expect("embed (no task) succeeded");
    let as_query = backend
        .embed(req_with(
            vec!["dragons breathe fire"],
            None,
            Some(EmbedTask::RetrievalQuery),
        ))
        .await
        .expect("embed (RetrievalQuery) succeeded");
    let as_doc = backend
        .embed(req_with(
            vec!["dragons breathe fire"],
            None,
            Some(EmbedTask::RetrievalDocument),
        ))
        .await
        .expect("embed (RetrievalDocument) succeeded");

    // Same text under different task prefixes should produce different
    // embeddings. We can't predict the magnitude of difference, but
    // cosine should fall short of 1.0.
    let c_query = cosine(&unprefixed.embeddings[0], &as_query.embeddings[0]);
    let c_doc = cosine(&unprefixed.embeddings[0], &as_doc.embeddings[0]);
    assert!(
        c_query < 0.999,
        "task prefix had no effect on the embedding (cos vs unprefixed = {c_query})"
    );
    assert!(
        c_doc < 0.999,
        "RetrievalDocument prefix had no effect (cos vs unprefixed = {c_doc})"
    );

    let c_query_doc = cosine(&as_query.embeddings[0], &as_doc.embeddings[0]);
    assert!(
        c_query_doc < 0.999,
        "RetrievalQuery and RetrievalDocument should differ (cos = {c_query_doc})"
    );
}

#[tokio::test]
async fn all_eight_task_prefixes_succeed() {
    let Some(path) = embed_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let tasks = [
        EmbedTask::RetrievalQuery,
        EmbedTask::RetrievalDocument,
        EmbedTask::Similarity,
        EmbedTask::Classification,
        EmbedTask::Clustering,
        EmbedTask::QuestionAnswering,
        EmbedTask::FactVerification,
        EmbedTask::CodeRetrievalQuery,
    ];
    for task in tasks {
        let r = backend
            .embed(req_with(vec!["sample text"], None, Some(task.clone())))
            .await
            .unwrap_or_else(|e| panic!("embed with task {task:?} failed: {e}"));
        assert_eq!(r.dimensions, 768);
        assert_eq!(r.embeddings[0].len(), 768);
        let n = l2_norm(&r.embeddings[0]);
        assert!(
            (n - 1.0).abs() < 1e-3,
            "task {task:?} produced non-unit-norm vector ({n})"
        );
    }
}
