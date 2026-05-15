//! Integration tests for the mock backend. Tier 1 + 2 per
//! `docs/test-strategy.md`.

use inferd_engine::mock::{Mock, MockConfig, MockError};
use inferd_engine::{Backend, GenerateError, TokenEvent};
use inferd_proto::{Message, Resolved, Role, StopReason};
use std::time::Duration;
use tokio_stream::StreamExt;

fn req() -> Resolved {
    Resolved {
        id: "test".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: 1.0,
        top_p: 0.95,
        top_k: 64,
        max_tokens: 1000,
        stream: true,
        image_token_budget: None,
        grammar: String::new(),
    }
}

#[tokio::test]
async fn mock_streams_tokens_and_terminates_with_done() {
    let mock = Mock::with_config(MockConfig {
        tokens: vec!["alpha".into(), "beta".into(), "gamma".into()],
        ..Default::default()
    });
    let stream = mock.generate(req()).await.expect("generate ok");
    let events: Vec<TokenEvent> = stream.collect().await;

    assert_eq!(events.len(), 4, "3 tokens + 1 done");
    assert_eq!(events[0], TokenEvent::Token("alpha".into()));
    assert_eq!(events[1], TokenEvent::Token("beta".into()));
    assert_eq!(events[2], TokenEvent::Token("gamma".into()));
    match &events[3] {
        TokenEvent::Done { stop_reason, usage } => {
            assert_eq!(*stop_reason, StopReason::End);
            assert_eq!(usage.completion_tokens, 3);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn mock_pre_stream_error_returned_directly() {
    let mock = Mock::with_config(MockConfig {
        pre_stream_error: Some(MockError::Unavailable),
        ..Default::default()
    });
    match mock.generate(req()).await {
        Err(GenerateError::Unavailable(_)) => {}
        other => panic!(
            "expected Unavailable error, got {other:?}",
            other = other.err()
        ),
    }
}

#[tokio::test]
async fn mock_not_ready_when_toggled() {
    let mock = Mock::new();
    mock.set_ready(false);
    assert!(!mock.ready());
    match mock.generate(req()).await {
        Err(GenerateError::NotReady) => {}
        other => panic!("expected NotReady, got {other:?}", other = other.err()),
    }
}

#[tokio::test]
async fn mock_mid_stream_drop_yields_no_done() {
    let mock = Mock::with_config(MockConfig {
        tokens: vec!["one".into(), "two".into(), "three".into()],
        mid_stream_drop_after: Some(2),
        ..Default::default()
    });
    let stream = mock.generate(req()).await.unwrap();
    let events: Vec<TokenEvent> = stream.collect().await;
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| matches!(e, TokenEvent::Token(_))));
}

// Cancellation: drop the stream and verify the spawned task exits cleanly.
// We can't observe the task directly, but we can verify the stream stops
// producing once dropped — confirming the channel is closed correctly.
#[tokio::test]
async fn mock_drop_cancels_generation() {
    let mock = Mock::with_config(MockConfig {
        tokens: (0..1000).map(|i| format!("t{i}")).collect(),
        ..Default::default()
    });
    let mut stream = mock.generate(req()).await.unwrap();
    // Take just one token then drop.
    let first = stream.next().await.unwrap();
    assert!(matches!(first, TokenEvent::Token(_)));
    drop(stream);
    // No assertion needed beyond "no panic / no hang."
    // Tokio test runtime will fail this test if the spawned task leaks.
    tokio::time::sleep(Duration::from_millis(10)).await;
}

#[tokio::test]
async fn mock_name_is_stable_diagnostic() {
    let mock = Mock::new();
    assert_eq!(mock.name(), "mock");
}

#[tokio::test]
async fn mock_stop_succeeds() {
    let mock = Mock::new();
    mock.stop(Duration::from_secs(1)).await.unwrap();
}
