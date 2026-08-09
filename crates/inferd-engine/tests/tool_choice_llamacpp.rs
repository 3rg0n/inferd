//! Tier-3 integration: `tool_choice` enforcement on a real model. Gated
//! on `llamacpp-integration` + INFERD_TEST_MODEL_PATH.
//!
//! These are the only tests that prove `tool_choice` is a *constraint*
//! rather than a hint. The unit tests cover grammar text and wire
//! mapping; neither can show that the sampler actually masks the tokens
//! that would let the model answer in prose. Two properties matter and
//! both need a real vocab + real sampling:
//!
//! - `required` must make a bare text answer **unreachable**. The
//!   grammar is installed eagerly and its root demands a complete call,
//!   so `llama_grammar_apply_impl` masks every EOG token until some
//!   stack is empty. A prompt deliberately chosen to invite prose ("just
//!   say hi") is the adversarial case: an advisory implementation
//!   answers it with prose, an enforced one cannot.
//!
//! - `none` must make a tool call unreachable even when the prompt
//!   begs for one, and must still terminate.
//!
//! `auto` is not asserted behaviourally — "the model decides" has no
//! observable invariant to pin. It is covered by the grammar-shape unit
//! tests and by the fact that it installs a lazy grammar at all.
#![cfg(feature = "llamacpp-integration")]

use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_engine::{Backend, TokenEventV2};
use inferd_proto::v2::{
    ContentBlock, MessageV2, RequestV2, ResponseFormat, RoleV2, Tool, ToolChoice,
};
use tokio_stream::StreamExt;

fn model_path() -> Option<std::path::PathBuf> {
    std::env::var_os("INFERD_TEST_MODEL_PATH").map(Into::into)
}

fn backend(n_ctx: u32) -> Option<LlamaCpp> {
    let path = model_path()?;
    Some(
        LlamaCpp::new(LlamaCppConfig {
            model_path: path,
            n_ctx,
            ..Default::default()
        })
        .expect("construct LlamaCpp"),
    )
}

fn weather_tool() -> Tool {
    Tool {
        name: "get_weather".into(),
        description: "Look up the current weather for a city.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
    }
}

/// Drain a stream into (text, tool-call names).
async fn drain(mut stream: inferd_engine::TokenStreamV2) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev {
            TokenEventV2::Text(t) => text.push_str(&t),
            TokenEventV2::ToolUse { name, .. } => calls.push(name),
            _ => {}
        }
    }
    (text, calls)
}

/// The load-bearing test: `required` against a prompt that invites prose.
/// An advisory `tool_choice` answers "just say hi" with a greeting; an
/// enforced one has no sampling path to it.
#[tokio::test]
async fn required_forces_a_tool_call_even_when_the_prompt_invites_prose() {
    let Some(backend) = backend(4096) else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };

    let req = RequestV2 {
        wire_version: 1,
        id: "tc-required".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "Do not use any tools. Just say hi.".into(),
            }],
        }],
        tools: vec![weather_tool()],
        tool_choice: Some(ToolChoice::Required),
        max_tokens: Some(128),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    let (text, calls) = drain(backend.generate_v2(req).await.expect("generate_v2")).await;
    eprintln!("=== required: text={text:?} calls={calls:?} ===");
    assert!(
        !calls.is_empty(),
        "tool_choice=required must yield a tool call; got text={text:?}"
    );
    assert_eq!(
        calls[0], "get_weather",
        "the grammar masks tool names to the declared table"
    );
}

/// `required` must also hold when several tools are declared: the name
/// alternation has to admit each of them, not just the first. A grammar
/// that hard-codes one name would still pass the test above.
#[tokio::test]
async fn required_admits_any_declared_tool() {
    let Some(backend) = backend(4096) else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };

    let clock = Tool {
        name: "get_time".into(),
        description: "Get the current time in a timezone.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"tz": {"type": "string"}},
            "required": ["tz"]
        }),
    };

    let req = RequestV2 {
        wire_version: 1,
        id: "tc-multi".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "What time is it in Tokyo?".into(),
            }],
        }],
        tools: vec![weather_tool(), clock],
        tool_choice: Some(ToolChoice::Required),
        max_tokens: Some(128),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    let (text, calls) = drain(backend.generate_v2(req).await.expect("generate_v2")).await;
    eprintln!("=== required multi: text={text:?} calls={calls:?} ===");
    assert!(
        !calls.is_empty(),
        "tool_choice=required must yield a tool call; got text={text:?}"
    );
    // Which one it picks is the model's call; that it is *one of the
    // declared two* is the grammar's.
    assert!(
        calls[0] == "get_time" || calls[0] == "get_weather",
        "called an undeclared tool: {:?}",
        calls[0]
    );
}

/// `none` must make a call unreachable against a prompt that begs for
/// one, and must still terminate. The exclusion automaton bars the
/// opener as *text*, so it holds however the opener is tokenised.
#[tokio::test]
async fn none_forbids_a_tool_call_even_when_the_prompt_demands_one() {
    let Some(backend) = backend(4096) else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };

    let req = RequestV2 {
        wire_version: 1,
        id: "tc-none".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "Call the get_weather tool for Paris. You must use the tool.".into(),
            }],
        }],
        tools: vec![weather_tool()],
        tool_choice: Some(ToolChoice::None),
        max_tokens: Some(128),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    let (text, calls) = drain(backend.generate_v2(req).await.expect("generate_v2")).await;
    eprintln!("=== none: text={text:?} calls={calls:?} ===");
    assert!(
        calls.is_empty(),
        "tool_choice=none must not yield a tool call; got {calls:?}"
    );
    assert!(
        !text.contains("<|tool_call>"),
        "the call opener reached the text channel: {text:?}"
    );
    assert!(
        !text.trim().is_empty(),
        "tool_choice=none must still produce an answer"
    );
}

/// `auto` must not break ordinary generation: the grammar is lazy, so a
/// request that doesn't want a tool answers in plain text. This is the
/// regression that a mis-specified lazy trigger would cause — an eager
/// grammar under `auto` would force a call here.
#[tokio::test]
async fn auto_leaves_a_plain_answer_reachable() {
    let Some(backend) = backend(4096) else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };

    let req = RequestV2 {
        wire_version: 1,
        id: "tc-auto".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "What is 2 + 2? Answer with just the number.".into(),
            }],
        }],
        tools: vec![weather_tool()],
        tool_choice: Some(ToolChoice::Auto),
        max_tokens: Some(64),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    let (text, calls) = drain(backend.generate_v2(req).await.expect("generate_v2")).await;
    eprintln!("=== auto: text={text:?} calls={calls:?} ===");
    assert!(
        calls.is_empty() && text.contains('4'),
        "auto must leave a plain text answer reachable; text={text:?} calls={calls:?}"
    );
}

/// `response_format` + `tool_choice` is rejected up front, not silently
/// downgraded. Only one grammar can be installed, so honouring either
/// would drop the other — and a dropped `required` is the fail-open the
/// field exists to close. Upstream llama.cpp drops the tool constraint
/// here; inferd refuses instead.
#[tokio::test]
async fn response_format_plus_tool_choice_is_rejected() {
    let Some(backend) = backend(2048) else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };

    let req = RequestV2 {
        wire_version: 1,
        id: "tc-clash".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "Weather in Paris?".into(),
            }],
        }],
        tools: vec![weather_tool()],
        tool_choice: Some(ToolChoice::Required),
        response_format: Some(ResponseFormat::JsonSchema {
            schema: serde_json::json!({"type":"object"}),
        }),
        max_tokens: Some(32),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    let err = backend
        .generate_v2(req)
        .await
        .err()
        .expect("must reject the pair");
    let msg = err.to_string();
    eprintln!("=== clash: {msg} ===");
    assert!(
        msg.contains("mutually exclusive"),
        "expected a mutual-exclusion error, got: {msg}"
    );
}
