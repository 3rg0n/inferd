//! Tier-3 integration: structured-output grammar (response_format) on a
//! real model. Gated on `llamacpp-integration` + INFERD_TEST_MODEL_PATH.
#![cfg(feature = "llamacpp-integration")]

use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_engine::{Backend, TokenEventV2};
use inferd_proto::v2::{ContentBlock, MessageV2, RequestV2, ResponseFormat, RoleV2};
use tokio_stream::StreamExt;

fn model_path() -> Option<std::path::PathBuf> {
    std::env::var_os("INFERD_TEST_MODEL_PATH").map(Into::into)
}

/// Thinking activation (#173) end-to-end on a real model: `thinking:true`
/// must make the model emit a reasoning trace that the parser separates
/// onto `Thinking` events — NOT leaked into user-visible `Text`. Proves
/// the renderer `<|think|>` activation + the GA `<|channel>thought…
/// <channel|>` parse fix work together against the real model.
#[tokio::test]
async fn thinking_activation_yields_thinking_events() {
    let Some(path) = model_path() else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };
    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 4096,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    // A small problem so the reasoning trace fits well within the token
    // budget and the `<channel|>` closer is reached (a prompt that
    // overflows max_tokens mid-thought would leave the block unclosed —
    // a different, valid path, but not what this test asserts).
    let req = RequestV2 {
        wire_version: 1,
        id: "think-1".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "What is 6 times 7? Reason briefly, then give the answer.".into(),
            }],
        }],
        max_tokens: Some(384),
        thinking: Some(true),
        ..Default::default()
    };

    let resolved = req.resolve().expect("resolve");
    let mut stream = backend.generate_v2(resolved).await.expect("generate_v2");
    let mut thinking = String::new();
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        match ev {
            TokenEventV2::Thinking(t) => thinking.push_str(&t),
            TokenEventV2::Text(t) => text.push_str(&t),
            _ => {}
        }
    }
    eprintln!("=== thinking ===\n{thinking}\n=== text ===\n{text}\n===");
    // The model should have produced a separated reasoning trace, and
    // the trace must NOT have leaked into the user-visible text (no raw
    // channel tokens there).
    assert!(
        !thinking.trim().is_empty(),
        "thinking=true should yield a separated reasoning trace; got none"
    );
    assert!(
        !text.contains("<|channel>") && !text.contains("<channel|>"),
        "raw channel tokens leaked into user-visible text: {text:?}"
    );
}

#[tokio::test]
async fn response_format_constrains_output_to_json() {
    let Some(path) = model_path() else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };
    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 4096,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    let schema = serde_json::json!({
        "type":"object",
        "properties":{"city":{"type":"string"},"population":{"type":"integer"}},
        "required":["city","population"],
        "additionalProperties":false
    });

    let req = RequestV2 {
        wire_version: 1,
        id: "g1".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "Tell me about the capital of France.".into(),
            }],
        }],
        max_tokens: Some(80),
        response_format: Some(ResponseFormat::JsonSchema { schema }),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    let mut stream = backend.generate_v2(req).await.expect("generate_v2");
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let TokenEventV2::Text(t) = ev {
            text.push_str(&t);
        }
    }
    eprintln!("=== constrained output ===\n{text}\n===");
    let parsed: serde_json::Value =
        serde_json::from_str(text.trim()).expect("constrained output must be valid JSON");
    assert!(parsed.get("city").is_some(), "must have city key");
    assert!(
        parsed.get("population").is_some(),
        "must have population key"
    );
}

/// Safety: a malformed/pathological schema must return a clean error, not
/// abort the daemon. A consumer-supplied schema crashing the host is a
/// DoS hole — the grammar path must fail closed.
#[tokio::test]
async fn malformed_schema_errors_does_not_crash() {
    let Some(path) = model_path() else {
        eprintln!("[skip] INFERD_TEST_MODEL_PATH not set");
        return;
    };
    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    // Not a valid JSON Schema object — a bare string. json_schema_to_grammar
    // should reject it; the daemon must surface an error, never crash.
    let schema = serde_json::json!("definitely not a schema");
    let req = RequestV2 {
        wire_version: 1,
        id: "bad".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        max_tokens: Some(16),
        response_format: Some(ResponseFormat::JsonSchema { schema }),
        ..Default::default()
    }
    .resolve()
    .expect("resolve");

    // Either generate_v2 returns Err, or the stream yields an Error event.
    // Crucially: the process must still be alive after this call.
    match backend.generate_v2(req).await {
        Err(_) => { /* rejected up front — good */ }
        Ok(mut stream) => {
            let mut saw_text = false;
            while let Some(ev) = stream.next().await {
                if let TokenEventV2::Text(_) = ev {
                    saw_text = true;
                }
            }
            // A bad schema must not silently produce free-text output.
            assert!(
                !saw_text,
                "malformed schema must not yield unconstrained text"
            );
        }
    }
}
