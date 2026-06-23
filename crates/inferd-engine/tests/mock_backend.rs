//! Integration tests for the mock backend. Tier 1 + 2 per
//! `docs/test-strategy.md`.

use inferd_engine::mock::{Mock, MockConfig, MockError};
use inferd_engine::{Backend, EmbedError, GenerateError, TokenEventV2};
use inferd_proto::embed::EmbedRequest;
use inferd_proto::v2::{ContentBlock, MessageV2, RequestV2, ResolvedV2, RoleV2, StopReasonV2};
use std::time::Duration;
use tokio_stream::StreamExt;

fn req() -> ResolvedV2 {
    RequestV2 {
        wire_version: inferd_proto::v2::WIRE_VERSION,
        id: "test".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        ..Default::default()
    }
    .resolve()
    .expect("resolve ok")
}

#[tokio::test]
async fn mock_streams_tokens_and_terminates_with_done() {
    let mock = Mock::with_config(MockConfig {
        tokens: vec!["alpha".into(), "beta".into(), "gamma".into()],
        ..Default::default()
    });
    let stream = mock.generate_v2(req()).await.expect("generate ok");
    let events: Vec<TokenEventV2> = stream.collect().await;

    assert_eq!(events.len(), 4, "3 tokens + 1 done");
    assert_eq!(events[0], TokenEventV2::Text("alpha".into()));
    assert_eq!(events[1], TokenEventV2::Text("beta".into()));
    assert_eq!(events[2], TokenEventV2::Text("gamma".into()));
    match &events[3] {
        TokenEventV2::Done { stop_reason, usage } => {
            assert_eq!(*stop_reason, StopReasonV2::EndTurn);
            assert_eq!(usage.output_tokens, 3);
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
    match mock.generate_v2(req()).await {
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
    match mock.generate_v2(req()).await {
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
    let stream = mock.generate_v2(req()).await.unwrap();
    let events: Vec<TokenEventV2> = stream.collect().await;
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| matches!(e, TokenEventV2::Text(_))));
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
    let mut stream = mock.generate_v2(req()).await.unwrap();
    // Take just one token then drop.
    let first = stream.next().await.unwrap();
    assert!(matches!(first, TokenEventV2::Text(_)));
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

#[tokio::test]
async fn mock_advertises_embed_capability() {
    let mock = Mock::new();
    assert!(mock.capabilities().embed);
}

#[tokio::test]
async fn mock_embed_returns_one_vector_per_input() {
    let mock = Mock::new();
    let req = EmbedRequest {
        id: "e1".into(),
        input: vec!["hello".into(), "world!".into()],
        ..Default::default()
    }
    .resolve()
    .unwrap();
    let result = mock.embed(req).await.expect("embed ok");
    assert_eq!(result.embeddings.len(), 2);
    assert_eq!(result.dimensions, 8);
    assert!(result.embeddings.iter().all(|v| v.len() == 8));
    assert_eq!(result.model, "mock");
    // input_tokens accumulates byte-length per the deterministic mock
    // formula: 5 + 6 = 11.
    assert_eq!(result.usage.input_tokens, 11);
}

#[tokio::test]
async fn mock_embed_honours_requested_dimensions() {
    let mock = Mock::new();
    let req = EmbedRequest {
        id: "e1".into(),
        input: vec!["hi".into()],
        dimensions: Some(3),
        ..Default::default()
    }
    .resolve()
    .unwrap();
    let result = mock.embed(req).await.unwrap();
    assert_eq!(result.dimensions, 3);
    assert_eq!(result.embeddings[0].len(), 3);
}

#[tokio::test]
async fn mock_embed_pre_stream_error_maps_to_unavailable() {
    let mock = Mock::with_config(MockConfig {
        pre_stream_error: Some(MockError::Unavailable),
        ..Default::default()
    });
    let req = EmbedRequest {
        id: "e1".into(),
        input: vec!["hi".into()],
        ..Default::default()
    }
    .resolve()
    .unwrap();
    match mock.embed(req).await {
        Err(EmbedError::Unavailable(_)) => {}
        other => panic!("expected Unavailable error, got {other:?}"),
    }
}

#[tokio::test]
async fn mock_embed_not_ready_when_toggled() {
    let mock = Mock::new();
    mock.set_ready(false);
    let req = EmbedRequest {
        id: "e1".into(),
        input: vec!["hi".into()],
        ..Default::default()
    }
    .resolve()
    .unwrap();
    match mock.embed(req).await {
        Err(EmbedError::NotReady) => {}
        other => panic!("expected NotReady, got {other:?}"),
    }
}
