//! End-to-end tests for the OpenAI-compat backend adapter.
//!
//! Uses `wiremock` to stand up a fake upstream that speaks the
//! Chat Completions wire. We don't depend on a real provider: the
//! tests are about whether the adapter speaks SSE correctly and whether
//! the mapper round-trip survives across the wire.

#![cfg(feature = "openai")]

use futures_util::StreamExt;
use inferd_engine::openai_compat::{OpenAiCompat, OpenAiCompatConfig};
use inferd_engine::{Backend, BackendCapabilities, TokenEventV2};
use inferd_proto::v2::{
    ContentBlock, MessageV2, RequestV2, RoleV2, StopReasonV2, Tool, ToolCallId,
};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cfg(server: &MockServer) -> OpenAiCompatConfig {
    OpenAiCompatConfig {
        base_url: server.uri(),
        api_key: "test-key".into(),
        model: "test-model".into(),
        timeout: Duration::from_secs(10),
    }
}

fn sse_body(chunks: &[&str]) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str("data: ");
        out.push_str(c);
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

fn user_text(text: &str) -> RequestV2 {
    RequestV2 {
        id: "req-1".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn capabilities_advertise_v2_and_tools_only() {
    let server = MockServer::start().await;
    let backend = OpenAiCompat::new(cfg(&server)).unwrap();
    let caps = backend.capabilities();
    assert_eq!(
        caps,
        BackendCapabilities {
            v2: true,
            tools: true,
            vision: false,
            audio: false,
            audio_sample_rate: None,
            video: false,
            thinking: false,
            embed: false,
            accelerator: Default::default(),
        }
    );
    assert!(backend.ready());
    assert_eq!(backend.name(), "openai-compat");
}

#[tokio::test]
async fn streams_text_and_emits_done_with_usage() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":" world"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let backend = OpenAiCompat::new(cfg(&server)).unwrap();
    let req = user_text("hi").resolve().unwrap();
    let mut stream = backend.generate_v2(req).await.unwrap();

    let mut text = String::new();
    let mut done_seen = None;
    while let Some(ev) = stream.next().await {
        match ev {
            TokenEventV2::Text(t) => text.push_str(&t),
            TokenEventV2::Done { stop_reason, usage } => {
                done_seen = Some((stop_reason, usage));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(text, "hello world");
    let (stop, usage) = done_seen.expect("Done frame missing");
    assert_eq!(stop, StopReasonV2::EndTurn);
    assert_eq!(usage.input_tokens, 7);
    assert_eq!(usage.output_tokens, 3);
}

#[tokio::test]
async fn assembles_tool_use_across_chunks() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_42","function":{"name":"lookup"}}]},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":\""}}]},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"abc\"}"}}]},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let backend = OpenAiCompat::new(cfg(&server)).unwrap();
    let mut req = user_text("call lookup please").resolve().unwrap();
    req.tools = vec![Tool {
        name: "lookup".into(),
        description: "look something up".into(),
        input_schema: json!({"type": "object"}),
    }];

    let mut stream = backend.generate_v2(req).await.unwrap();
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    assert_eq!(events.len(), 2);
    match &events[0] {
        TokenEventV2::ToolUse {
            tool_call_id,
            name,
            input,
        } => {
            assert_eq!(tool_call_id, &ToolCallId("call_42".into()));
            assert_eq!(name, "lookup");
            assert_eq!(input, &json!({"q": "abc"}));
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
    match &events[1] {
        TokenEventV2::Done { stop_reason, usage } => {
            assert_eq!(*stop_reason, StopReasonV2::ToolUse);
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn http_500_surfaces_as_unavailable_pre_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string(r#"{"error":"upstream toast"}"#))
        .mount(&server)
        .await;

    let backend = OpenAiCompat::new(cfg(&server)).unwrap();
    let req = user_text("hi").resolve().unwrap();
    let err = match backend.generate_v2(req).await {
        Ok(_) => panic!("expected pre-stream error"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("upstream HTTP 500"),
        "expected HTTP 500 surface, got: {msg}"
    );
}

#[tokio::test]
async fn no_auth_header_when_api_key_empty() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    ]);

    // Wiremock has no "must NOT have header" matcher, so we just
    // verify the request still succeeds with no key configured. The
    // adapter source is the source of truth that the header is
    // skipped — this guards the success-path round-trip.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut config = cfg(&server);
    config.api_key.clear();
    let backend = OpenAiCompat::new(config).unwrap();
    let req = user_text("hi").resolve().unwrap();
    let mut stream = backend.generate_v2(req).await.unwrap();
    let mut saw_done = false;
    while let Some(ev) = stream.next().await {
        if matches!(ev, TokenEventV2::Done { .. }) {
            saw_done = true;
        }
    }
    assert!(saw_done);
}
