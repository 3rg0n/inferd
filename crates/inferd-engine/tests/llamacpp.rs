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
use inferd_engine::{Backend, TokenEvent};
use inferd_proto::{Message, Resolved, Role, StopReason};
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

fn req(text: &str) -> Resolved {
    Resolved {
        id: "t1".into(),
        messages: vec![Message {
            role: Role::User,
            content: text.into(),
        }],
        temperature: 0.7,
        top_p: 0.95,
        top_k: 40,
        max_tokens: 16,
        stream: true,
        image_token_budget: None,
        grammar: String::new(),
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
        .generate(req("Say hi briefly."))
        .await
        .expect("generate");
    let events: Vec<TokenEvent> = tokio::time::timeout(Duration::from_secs(60), stream.collect())
        .await
        .expect("generation timed out");

    assert!(!events.is_empty(), "expected at least a Done event");

    let last = events.last().unwrap();
    match last {
        TokenEvent::Done { stop_reason, usage } => {
            assert!(matches!(*stop_reason, StopReason::End | StopReason::Length));
            assert!(
                usage.completion_tokens > 0,
                "expected completion_tokens > 0, got {}",
                usage.completion_tokens
            );
        }
        other => panic!("expected terminal Done event, got {other:?}"),
    }

    let token_count = events
        .iter()
        .filter(|e| matches!(e, TokenEvent::Token(_)))
        .count();
    assert!(token_count > 0, "expected at least one Token event");
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
        .generate({
            let mut r = req("Tell me a long story about a dragon.");
            r.max_tokens = 200;
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

#[tokio::test]
async fn rejects_invalid_messages() {
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

    // Empty messages would normally be caught by Resolved-time validation,
    // but the type allows it. The chat template render returns None, which
    // surfaces as InvalidRequest from generate().
    let mut r = req("hello");
    r.messages.clear();

    let result = backend.generate(r).await;
    assert!(
        matches!(
            result.as_ref().err(),
            Some(inferd_engine::GenerateError::InvalidRequest(_))
        ),
        "expected InvalidRequest, got {:?}",
        result.err()
    );
}
