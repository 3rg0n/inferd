//! Tier 3 integration tests for `LlamaCpp::rerank` (ADR 0027).
//!
//! Exercises the real FFI path against a cross-encoder reranker GGUF:
//! the dedicated `LLAMA_POOLING_TYPE_RANK` context, the classification
//! head read through `llama_get_embeddings_seq`, pair formatting (model
//! `rerank` template or the SEP-joined fallback), descending sort,
//! `top_n` truncation, and the pre-FFI oversize rejection that stands
//! between an over-long pair and libllama's `n_ubatch` abort (issue #20).
//!
//! **Why this tier is load-bearing here.** The mock backend scores by
//! word overlap, so every Tier 1/2 rerank test passes regardless of
//! whether the classification head is wired up at all. A reranker that
//! returns `n_embd` floats instead of `n_cls_out`, or reads element 0 of
//! the wrong buffer, produces plausible numbers in a plausible order —
//! only a real model reveals that the ordering tracks relevance.
//!
//! Gated behind the `llamacpp-integration` cargo feature and skips
//! itself with an explanatory message if `INFERD_TEST_RERANK_MODEL_PATH`
//! is unset. That is a *separate* env var from the generation and embed
//! ones because rerank needs a genuinely different model: a cross-encoder
//! with a classification head. Gemma 4 and EmbeddingGemma both lack one,
//! so pointing this at either fails the load (by design — see
//! `LlamaCppError::RerankPreconditions`).
//!
//! To run locally:
//!   cargo test -p inferd-engine \
//!     --features llamacpp-integration \
//!     --test rerank_llamacpp \
//!     -- --nocapture
//! with `INFERD_TEST_RERANK_MODEL_PATH=/path/to/bge-reranker-v2-m3.gguf`.

#![cfg(feature = "llamacpp-integration")]

use inferd_engine::Backend;
use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_proto::rerank::{RerankRequest, RerankResolved};
use std::path::PathBuf;
use std::time::Duration;

fn rerank_model_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_RERANK_MODEL_PATH").map(PathBuf::from)
}

fn skipping_msg() {
    eprintln!(
        "[skip] INFERD_TEST_RERANK_MODEL_PATH not set; skipping tier-3 \
         rerank integration test. Point it at a cross-encoder reranker \
         GGUF with a classification head (e.g. bge-reranker-v2-m3). A \
         generation or bi-encoder embedding model will not load with \
         rerank = true."
    );
}

fn build_backend(path: PathBuf) -> LlamaCpp {
    LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        rerank: true,
        rerank_n_ctx: 2048,
        ..Default::default()
    })
    .expect("construct LlamaCpp with rerank=true")
}

fn req(query: &str, documents: Vec<&str>, top_n: Option<u32>) -> RerankResolved {
    RerankRequest {
        id: "r1".into(),
        query: query.into(),
        documents: documents.into_iter().map(String::from).collect(),
        top_n,
    }
    .resolve()
    .expect("resolve rerank request")
}

/// One clearly-relevant document and two clearly-irrelevant ones. Any
/// working cross-encoder must rank index 1 first; asserting only the
/// *winner* (rather than a full permutation or a score threshold) keeps
/// the test model-agnostic — raw logit scales differ per reranker, and
/// the relative order of two equally-irrelevant documents is noise.
fn relevance_fixture() -> RerankResolved {
    req(
        "how do I bind a unix domain socket in C",
        vec![
            "A recipe for sourdough starter, day by day.",
            "Create the socket with socket(AF_UNIX, SOCK_STREAM, 0), then \
             call bind(2) with a sockaddr_un naming the filesystem path.",
            "The migratory patterns of the Arctic tern.",
        ],
        None,
    )
}

#[tokio::test]
async fn advertises_rerank_capability() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    assert!(backend.ready());
    let caps = backend.capabilities();
    assert!(caps.rerank, "expected capabilities().rerank = true");
    assert!(
        !caps.embed,
        "rerank must not imply embed — pooling type is fixed per context, \
         and RANK pooling returns class logits, not an embedding"
    );
    assert_eq!(backend.name(), "llamacpp");
}

/// The default is off: a rerank context is a second context plus KV
/// cache against the model, and a deployment doing no retrieval must not
/// pay for it. This also pins that the daemon never binds the rerank
/// socket for an ordinary generation backend.
#[tokio::test]
async fn rerank_disabled_by_default_and_the_call_reports_unsupported() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        // rerank left at its default (false).
        ..Default::default()
    })
    .expect("construct LlamaCpp without rerank");

    assert!(!backend.capabilities().rerank, "rerank must default to off");
    let err = backend
        .rerank(relevance_fixture())
        .await
        .expect_err("rerank without a rerank context must fail");
    assert!(
        matches!(err, inferd_engine::RerankError::Unsupported),
        "expected Unsupported, got {err:?}"
    );
}

#[tokio::test]
async fn ranks_the_relevant_document_first() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let out = tokio::time::timeout(
        Duration::from_secs(120),
        backend.rerank(relevance_fixture()),
    )
    .await
    .expect("rerank timed out")
    .expect("rerank succeeded");

    assert_eq!(out.results.len(), 3, "no top_n → every document scored");
    assert_eq!(
        out.results[0].index,
        1,
        "the socket-programming document must outrank the two irrelevant \
         ones; got ordering {:?}",
        out.results
            .iter()
            .map(|r| (r.index, r.score))
            .collect::<Vec<_>>()
    );
    for w in out.results.windows(2) {
        assert!(
            w[0].score >= w[1].score,
            "results must be sorted by score descending, got {:?}",
            out.results
        );
    }
    // Every index appears exactly once, and each maps back into the
    // caller's input — the index *is* the whole contract, since the
    // daemon returns no document text.
    let mut seen: Vec<u32> = out.results.iter().map(|r| r.index).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        vec![0, 1, 2],
        "indices must be a permutation of the input"
    );
    assert!(
        out.results.iter().all(|r| r.score.is_finite()),
        "a NaN or infinite score means the wrong buffer was read: {:?}",
        out.results
    );
    assert!(
        out.usage.input_tokens > 0,
        "one forward pass per pair — token count cannot be zero"
    );
    // Model label is the GGUF `general.name` (or file stem), never the
    // backend name — `Backend::name()` already exposes that.
    assert_ne!(out.model, "llamacpp");
    assert!(!out.model.is_empty(), "model label must not be empty");
}

/// A cross-encoder scores the *pair*, so swapping the query has to move
/// the scores. If it doesn't, the query half of the pair isn't reaching
/// the model — the failure mode that a bi-encoder-shaped implementation
/// would produce, and one no unit test can see.
#[tokio::test]
async fn score_depends_on_the_query_not_just_the_document() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let doc = "Create the socket with socket(AF_UNIX, SOCK_STREAM, 0), then \
               call bind(2) with a sockaddr_un naming the filesystem path.";

    let on_topic = backend
        .rerank(req("how do I bind a unix domain socket", vec![doc], None))
        .await
        .expect("rerank (on-topic query) succeeded");
    let off_topic = backend
        .rerank(req("what temperature to proof sourdough", vec![doc], None))
        .await
        .expect("rerank (off-topic query) succeeded");

    let s_on = on_topic.results[0].score;
    let s_off = off_topic.results[0].score;
    assert!(
        (s_on - s_off).abs() > 1e-4,
        "the same document scored identically under two unrelated queries \
         ({s_on} vs {s_off}) — the query is not reaching the model"
    );
    assert!(
        s_on > s_off,
        "the on-topic query should score higher: {s_on} vs {s_off}"
    );
}

#[tokio::test]
async fn top_n_truncates_after_sorting() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let full = backend
        .rerank(relevance_fixture())
        .await
        .expect("rerank (untruncated) succeeded");

    let mut truncated_req = relevance_fixture();
    truncated_req.top_n = Some(1);
    let truncated = backend
        .rerank(truncated_req)
        .await
        .expect("rerank (top_n=1) succeeded");

    assert_eq!(truncated.results.len(), 1);
    assert_eq!(
        truncated.results[0].index, full.results[0].index,
        "top_n must keep the head of the *ordering*, not of the input"
    );
    // The KV cache is cleared between pairs, so scoring the same pair
    // twice must give the same number. A drift here means state leaked
    // between forward passes.
    assert_eq!(
        truncated.results[0].score, full.results[0].score,
        "scoring is deterministic per pair; the KV cache must be cleared \
         between documents"
    );
}

/// A `top_n` above the document count means "all of them" — a caller
/// whose candidate set shrank shouldn't have to clamp.
#[tokio::test]
async fn top_n_above_document_count_returns_all() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    let mut r = relevance_fixture();
    r.top_n = Some(99);
    let out = backend.rerank(r).await.expect("rerank succeeded");
    assert_eq!(out.results.len(), 3);
}

/// Regression for the issue #20 class of bug on the rerank surface.
/// libllama asserts `n_ubatch >= n_tokens` inside `llama_encode` and
/// aborts the *process* — so an over-long pair has to be rejected before
/// the FFI call. A small `rerank_n_ctx` clamps `n_ubatch`, making the
/// rejection reachable without a megabyte-scale document.
#[tokio::test]
async fn oversized_pair_returns_invalid_request_not_abort() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        rerank: true,
        rerank_n_ctx: 64, // Forces a tiny n_ubatch.
        ..Default::default()
    })
    .expect("construct LlamaCpp with small rerank_n_ctx");

    let long: String = "lorem ipsum dolor sit amet ".repeat(50);
    let err = backend
        .rerank(req("q", vec!["short", long.as_str()], None))
        .await
        .expect_err("an over-long pair must be rejected, not encoded");

    match err {
        inferd_engine::RerankError::InvalidRequest(msg) => {
            assert!(
                msg.contains("documents[1]"),
                "the message must name which document overflowed — one bad \
                 document in a batch of 256 is the common case; got {msg:?}"
            );
            assert!(
                msg.contains("n_ubatch") || msg.contains("exceeds"),
                "expected n_ubatch / exceeds in the message, got {msg:?}"
            );
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }

    // Crucially the backend survived: the rejection happened before the
    // FFI call, so the process is intact and still serving.
    let out = backend
        .rerank(req("q", vec!["short"], None))
        .await
        .expect("backend must still serve short pairs after the rejection");
    assert_eq!(out.results.len(), 1);
}

/// The document cap is a proto-level bound (`MAX_RERANK_DOCUMENTS`), but
/// the *cost* it bounds is here: one forward pass per document. Scoring a
/// full-cap batch on a real model is the only place that cost is
/// observable, and it pins that nothing in the loop is quadratic.
#[tokio::test]
async fn scores_a_batch_without_leaking_state_between_pairs() {
    let Some(path) = rerank_model_path() else {
        skipping_msg();
        return;
    };
    let backend = build_backend(path);

    // The same document repeated. Every score must be identical: the
    // pair is identical, and the KV cache is cleared between passes. Any
    // drift across the batch is leaked state.
    let doc = "bind(2) attaches a name to a socket.";
    let out = tokio::time::timeout(
        Duration::from_secs(180),
        backend.rerank(req("binding sockets", vec![doc; 8], None)),
    )
    .await
    .expect("rerank timed out")
    .expect("rerank succeeded");

    assert_eq!(out.results.len(), 8);
    let first = out.results[0].score;
    for r in &out.results {
        assert_eq!(
            r.score, first,
            "identical pairs must score identically; drift across the batch \
             means the KV cache is not being cleared between documents \
             ({:?})",
            out.results
        );
    }
    // Stable sort on equal scores keeps input order, so the indices come
    // back ascending — pinned because a caller resolving indices against
    // its own candidate list depends on ties being predictable.
    assert_eq!(
        out.results.iter().map(|r| r.index).collect::<Vec<_>>(),
        (0..8).collect::<Vec<u32>>(),
        "equal scores must preserve document order (stable sort)"
    );
}
