//! Tier 3 integration tests for the `LlamaCpp` adapter.
//!
//! Per `docs/test-strategy.md` §"Tier 3", these run end-to-end against a
//! real `libllama` build and an on-disk GGUF model. They are gated behind
//! the `llamacpp-integration` cargo feature and skip themselves with an
//! explanatory message if `INFERD_TEST_MODEL_PATH` is unset.
//!
//! To run locally:
//!   cargo test -p inferd-engine \
//!     --features llamacpp-integration \
//!     --test llamacpp \
//!     -- --nocapture
//! with `INFERD_TEST_MODEL_PATH=/path/to/gemma-4-e2b.Q4_K_M.gguf` set.

#![cfg(feature = "llamacpp-integration")]

use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_engine::{Backend, TokenEventV2};
use inferd_proto::v2::{ContentBlock, MessageV2, ResolvedV2, RoleV2, StopReasonV2};
use std::path::PathBuf;
use std::time::Duration;
use tokio_stream::StreamExt;

fn model_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_MODEL_PATH").map(PathBuf::from)
}

fn skipping_msg() {
    eprintln!(
        "[skip] INFERD_TEST_MODEL_PATH not set; skipping tier-3 llamacpp \
         integration test. See docs/test-strategy.md."
    );
}

// Build a ResolvedV2 directly (fields are pub) so the
// `rejects_invalid_messages` test can hand the backend an
// intentionally-empty message list that `RequestV2::resolve` would
// otherwise reject before it reached the engine.
fn req(text: &str) -> ResolvedV2 {
    ResolvedV2 {
        wire_version: inferd_proto::v2::WIRE_VERSION,
        id: "t1".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }],
        attachments: Vec::new(),
        tools: Vec::new(),
        tool_choice: None,
        temperature: Some(0.7),
        top_p: Some(0.95),
        top_k: Some(40),
        max_tokens: Some(16),
        stream: Some(true),
        response_format: None,
        thinking: None,
    }
}

#[tokio::test]
async fn loads_model_and_streams_tokens() {
    let Some(path) = model_path() else {
        skipping_msg();
        return;
    };

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    assert_eq!(backend.name(), "llamacpp");
    assert!(backend.ready());

    let stream = backend
        .generate_v2(req("Say hi briefly."))
        .await
        .expect("generate");
    let events: Vec<TokenEventV2> = tokio::time::timeout(Duration::from_secs(60), stream.collect())
        .await
        .expect("generation timed out");

    assert!(!events.is_empty(), "expected at least a Done event");

    let last = events.last().unwrap();
    match last {
        TokenEventV2::Done { stop_reason, usage } => {
            assert!(matches!(
                *stop_reason,
                StopReasonV2::EndTurn | StopReasonV2::MaxTokens
            ));
            assert!(
                usage.output_tokens > 0,
                "expected output_tokens > 0, got {}",
                usage.output_tokens
            );
        }
        other => panic!("expected terminal Done event, got {other:?}"),
    }

    let token_count = events
        .iter()
        .filter(|e| matches!(e, TokenEventV2::Text(_)))
        .count();
    assert!(token_count > 0, "expected at least one Text event");
}

#[tokio::test]
async fn cancellation_stops_generation_promptly() {
    let Some(path) = model_path() else {
        skipping_msg();
        return;
    };

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    let stream = backend
        .generate_v2({
            let mut r = req("Tell me a long story about a dragon.");
            r.max_tokens = Some(200);
            r
        })
        .await
        .expect("generate");

    // Take 1 token, then drop the stream — generation should stop without
    // panicking and without waiting for max_tokens.
    let mut s = stream;
    let first = tokio::time::timeout(Duration::from_secs(60), s.next())
        .await
        .expect("first token timed out");
    assert!(first.is_some());
    drop(s);

    // No assertion beyond "doesn't hang." If the spawn_blocking task
    // never noticed the cancel, this test would leak it; tokio-test
    // will not panic but the suite as a whole would slow as more leaked
    // tasks accumulate. A noticeable signal during local runs.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// An empty `messages[]` is rejected at the **proto boundary**, which
/// is the only place it can be rejected: `ResolvedV2` is constructible
/// only by `RequestV2::resolve()` outside of tests, so by the time the
/// engine sees one the check has already run.
///
/// This test used to assert that `generate_v2` returned
/// `InvalidRequest` for an empty message list, on the strength of a
/// renderer that returned `Option` and yielded `None` for it. That
/// renderer is gone (the `ChatRenderer` trait returns `Result`, and no
/// implementor treats "no messages" as an error), so the assertion was
/// testing an API that no longer exists — it just never reported,
/// because the Tier 3 binary crashed at teardown before printing a
/// summary (issue #202).
///
/// Asserting against the real gate instead of re-adding an engine-layer
/// check keeps the validation at the system boundary where it belongs.
/// If `resolve()` ever stops rejecting this, this test fails rather
/// than the burden silently shifting to a renderer that doesn't carry
/// it.
#[test]
fn empty_messages_are_rejected_at_the_proto_boundary() {
    use inferd_proto::v2::RequestV2;

    let json = serde_json::json!({
        "type": "generate",
        "wire_version": inferd_proto::v2::WIRE_VERSION,
        "id": "t-empty",
        "messages": [],
    });
    let req: RequestV2 = serde_json::from_value(json).expect("parses as a RequestV2");
    let err = req.resolve().expect_err("empty messages must not resolve");
    assert!(
        err.to_string().contains("messages must not be empty"),
        "got: {err}"
    );
}

/// A request the engine *does* reject: `max_tokens: 0` leaves nothing
/// to generate. Exercises the `generate_v2` rejection path that the
/// old empty-messages test was standing in for.
#[tokio::test]
async fn rejects_zero_max_tokens() {
    let Some(path) = model_path() else {
        skipping_msg();
        return;
    };

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 1024,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    let mut r = req("hello");
    r.max_tokens = Some(0);

    let stream = backend.generate_v2(r).await.expect("generate");
    let events: Vec<TokenEventV2> = tokio::time::timeout(Duration::from_secs(60), stream.collect())
        .await
        .expect("generation timed out");

    // Nothing to generate: terminate immediately on MaxTokens with no
    // text, rather than running the decode loop or hanging.
    //
    // Asserting the event list is *exactly* one terminal frame — not just
    // that the last frame looks right — is what pins "the sampler never
    // ran". An off-by-one in the decode loop bound would still report
    // MaxTokens, so a last-frame-only check would pass while the engine
    // emitted a token it was told not to.
    let last = events.last().expect("expected a terminal event");
    match last {
        TokenEventV2::Done { stop_reason, usage } => {
            assert_eq!(*stop_reason, StopReasonV2::MaxTokens);
            assert_eq!(usage.output_tokens, 0);
        }
        other => panic!("expected terminal Done event, got {other:?}"),
    }
    assert_eq!(
        events.len(),
        1,
        "max_tokens=0 must emit only the terminal frame, got {events:?}"
    );
}
