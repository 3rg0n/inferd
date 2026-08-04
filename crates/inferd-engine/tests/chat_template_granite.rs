//! Byte-exact reference-output tests for the IBM Granite chat-template
//! renderer.
//!
//! Source of truth: `granite-docling-258M/chat_template.jinja`,
//! corroborated by llama.cpp's `LLM_CHAT_TEMPLATE_GRANITE_3_X`
//! (`vendor/llama.cpp/src/llama-chat.cpp:631`).
//!
//! Granite is the second implementor of `ChatRenderer` (ADR 0026) and
//! exists to prove the seam: these tests assert the *differences* from
//! Gemma 4 — no literal BOS, no system-turn special case, no tool
//! grammar — are real, so a future refactor cannot collapse the two
//! renderers back into one.

#![cfg(feature = "llamacpp")]

use inferd_engine::llamacpp::{ChatRenderer, GraniteRenderer, RenderError};
use inferd_proto::v2::{Attachment, ContentBlock, MessageV2, RequestV2, RoleV2, Tool, ToolCallId};
use serde_json::json;

fn render(req: RequestV2) -> (String, usize) {
    let resolved = req.resolve().expect("request must resolve");
    let renderer = GraniteRenderer::new();
    let r = renderer.render(&resolved).expect("must render");
    (r.prompt, r.attachments.len())
}

fn render_err(req: RequestV2) -> RenderError {
    let resolved = req.resolve().expect("request must resolve");
    let renderer = GraniteRenderer::new();
    renderer.render(&resolved).expect_err("must reject")
}

fn user(text: &str) -> MessageV2 {
    MessageV2 {
        role: RoleV2::User,
        content: vec![ContentBlock::Text { text: text.into() }],
    }
}

#[test]
fn text_only_user_turn() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![user("What's in this document?")],
        ..Default::default()
    };
    let (out, n_atts) = render(req);
    assert_eq!(n_atts, 0);
    let expected = "<|start_of_role|>user<|end_of_role|>\
What's in this document?<|end_of_text|>\n\
<|start_of_role|>assistant<|end_of_role|>";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn no_literal_bos_token() {
    // Granite's template emits no BOS string; the tokenizer adds the
    // BOS token itself (`add_special = true`), so a literal one here
    // would double it. Guards against copy-pasting Gemma's `<bos>`.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![user("hi")],
        ..Default::default()
    };
    let (out, _) = render(req);
    assert!(!out.contains("<bos>"), "unexpected literal BOS in:\n{out}");
    assert!(
        out.starts_with("<|start_of_role|>"),
        "prompt must open on the first role tag; got:\n{out}"
    );
}

#[test]
fn system_is_an_ordinary_turn() {
    // Unlike Gemma, no synthesised turn and no `<|think|>` injection —
    // a system message is just another role.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::System,
                content: vec![ContentBlock::Text {
                    text: "You are a helpful assistant.".into(),
                }],
            },
            user("hi"),
        ],
        ..Default::default()
    };
    let (out, _) = render(req);
    let expected = "<|start_of_role|>system<|end_of_role|>\
You are a helpful assistant.<|end_of_text|>\n\
<|start_of_role|>user<|end_of_role|>\
hi<|end_of_text|>\n\
<|start_of_role|>assistant<|end_of_role|>";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn assistant_turn_is_named_assistant_not_model() {
    // Gemma calls the assistant turn "model". Granite does not.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            user("hi"),
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            },
            user("again"),
        ],
        ..Default::default()
    };
    let (out, _) = render(req);
    assert!(!out.contains("<|start_of_role|>model<"), "got:\n{out}");
    let expected = "<|start_of_role|>user<|end_of_role|>hi<|end_of_text|>\n\
<|start_of_role|>assistant<|end_of_role|>hello<|end_of_text|>\n\
<|start_of_role|>user<|end_of_role|>again<|end_of_text|>\n\
<|start_of_role|>assistant<|end_of_role|>";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn image_attachment_emits_shared_media_marker() {
    // The marker is mtmd's, not Gemma's — the media path generalises
    // across families for free. Granite-docling's own jinja writes a
    // literal `<image>`; emitting it here would double the IDEFICS3
    // fences mtmd_tokenize substitutes for the marker.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![
                ContentBlock::Text {
                    text: "Convert this page.".into(),
                },
                ContentBlock::Image {
                    attachment_id: "img-1".into(),
                },
            ],
        }],
        attachments: vec![Attachment::Image {
            id: "img-1".into(),
            width: 32,
            height: 32,
            bytes: "Zm9v".into(),
        }],
        ..Default::default()
    };
    let resolved = req.resolve().unwrap();
    let rendered = GraniteRenderer::new().render(&resolved).unwrap();

    assert_eq!(rendered.attachments.len(), 1);
    assert_eq!(rendered.attachments[0].id(), "img-1");
    assert!(
        !rendered.prompt.contains("<image>"),
        "got:\n{}",
        rendered.prompt
    );
    let expected = "<|start_of_role|>user<|end_of_role|>\
Convert this page.<__media__><|end_of_text|>\n\
<|start_of_role|>assistant<|end_of_role|>";
    assert_eq!(rendered.prompt, expected, "got:\n{}", rendered.prompt);
}

#[test]
fn attachment_order_follows_block_order() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![
                ContentBlock::Image {
                    attachment_id: "a".into(),
                },
                ContentBlock::Text {
                    text: " then ".into(),
                },
                ContentBlock::Image {
                    attachment_id: "b".into(),
                },
            ],
        }],
        attachments: vec![
            Attachment::Image {
                id: "b".into(),
                width: 8,
                height: 8,
                bytes: "1".into(),
            },
            Attachment::Image {
                id: "a".into(),
                width: 8,
                height: 8,
                bytes: "2".into(),
            },
        ],
        ..Default::default()
    };
    let resolved = req.resolve().unwrap();
    let rendered = GraniteRenderer::new().render(&resolved).unwrap();
    // Declaration order in `attachments[]` is deliberately the reverse
    // of block order: the renderer must follow the *blocks*, since mtmd
    // consumes attachments positionally against the markers.
    let ids: Vec<&str> = rendered.attachments.iter().map(|a| a.id()).collect();
    assert_eq!(ids, vec!["a", "b"]);
}

#[test]
fn dangling_attachment_never_reaches_the_renderer() {
    // `RenderError::DanglingAttachment` is the trait's error contract,
    // but the proto layer is the actual gate: `resolve()` refuses a
    // block referencing an undeclared id, so no renderer ever sees one.
    // Asserting that here keeps the belt honest — if resolve ever stops
    // checking, this fails rather than silently shifting the burden.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Image {
                attachment_id: "nope".into(),
            }],
        }],
        ..Default::default()
    };
    let err = req.resolve().expect_err("resolve must reject");
    assert!(format!("{err}").contains("nope"), "got: {err}");
}

#[test]
fn tool_declarations_are_rejected_not_dropped() {
    // The whole point of ADR 0026: a family with no tool grammar must
    // refuse rather than answer fluently without ever being told the
    // tools exist.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![user("what's the weather?")],
        tools: vec![Tool {
            name: "get_current_temperature".into(),
            description: "Gets the current temperature.".into(),
            input_schema: json!({"type": "OBJECT"}),
        }],
        ..Default::default()
    };
    match render_err(req) {
        RenderError::Unsupported { feature, .. } => assert_eq!(feature, "tool declarations"),
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn tool_use_block_is_rejected() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::Assistant,
            content: vec![ContentBlock::ToolUse {
                tool_call_id: ToolCallId::from("tc-1"),
                name: "get_current_weather".into(),
                input: json!({"location": "Tokyo, JP"}),
            }],
        }],
        ..Default::default()
    };
    match render_err(req) {
        RenderError::Unsupported { feature, .. } => assert_eq!(feature, "tool_use content blocks"),
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn tool_result_block_is_rejected() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::Assistant,
            content: vec![ContentBlock::ToolResult {
                tool_call_id: ToolCallId::from("tc-1"),
                content: vec![ContentBlock::Text {
                    text: "{\"temperature\":15}".into(),
                }],
            }],
        }],
        ..Default::default()
    };
    match render_err(req) {
        RenderError::Unsupported { feature, .. } => {
            assert_eq!(feature, "tool_result content blocks")
        }
        other => panic!("wrong error: {other}"),
    }
}

#[test]
fn thinking_true_is_rejected_but_false_renders() {
    let asking = RequestV2 {
        id: "x".into(),
        messages: vec![user("hi")],
        thinking: Some(true),
        ..Default::default()
    };
    match render_err(asking) {
        RenderError::Unsupported { feature, .. } => assert_eq!(feature, "thinking mode"),
        other => panic!("wrong error: {other}"),
    }

    // An explicit `false` is satisfiable by rendering nothing extra.
    let declining = RequestV2 {
        id: "x".into(),
        messages: vec![user("hi")],
        thinking: Some(false),
        ..Default::default()
    };
    let (out, _) = render(declining);
    assert!(!out.contains("<|think|>"), "got:\n{out}");
}

#[test]
fn capabilities_are_reported_honestly() {
    let r = GraniteRenderer::new();
    assert!(!r.supports_tools(), "Granite has no tool grammar");
    assert!(!r.supports_thinking(), "Granite has no thinking token");
}
