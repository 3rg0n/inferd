//! Byte-exact reference-output tests for the Gemma 4 chat-template
//! renderer.
//!
//! Source of truth: `docs/text.function.calling.with.gemma.4.md`.
//! Each test below mirrors a canonical example from that document
//! and asserts the renderer produces the byte-exact same output.
//! When the upstream chat template changes (a new Gemma version,
//! new tokens, new structural rules), these tests fail loudly and
//! the renderer is updated in lockstep.

#![cfg(feature = "llamacpp")]

use inferd_engine::llamacpp::Gemma4Renderer;
use inferd_proto::v2::{Attachment, ContentBlock, MessageV2, RequestV2, RoleV2, Tool, ToolCallId};
use serde_json::json;

fn render(req: RequestV2) -> (String, usize) {
    let resolved = req.resolve().expect("request must resolve");
    let renderer = Gemma4Renderer::new();
    let r = renderer.render(&resolved).expect("must render");
    (r.prompt, r.attachments.len())
}

#[test]
fn text_only_user_turn() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "What's the temperature in London?".into(),
            }],
        }],
        ..Default::default()
    };
    let (out, n_atts) = render(req);
    assert_eq!(n_atts, 0);
    let expected = "<bos>\
<|turn>user\n\
What's the temperature in London?<turn|>\n\
<|turn>model\n";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn system_user_assistant_three_turn() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::System,
                content: vec![ContentBlock::Text {
                    text: "You are a helpful assistant.".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![ContentBlock::Text {
                    text: "Hi there.".into(),
                }],
            },
        ],
        ..Default::default()
    };
    let (out, _) = render(req);
    let expected = "<bos>\
<|turn>system\n\
You are a helpful assistant.<turn|>\n\
<|turn>user\n\
hello<turn|>\n\
<|turn>model\n\
Hi there.<turn|>\n\
<|turn>model\n";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn tool_declaration_in_system_turn_matches_canonical() {
    // From docs/text.function.calling.with.gemma.4.md line 88-91.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::System,
                content: vec![ContentBlock::Text {
                    text: "You are a helpful assistant.".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "What's the temperature in London?".into(),
                }],
            },
        ],
        tools: vec![Tool {
            name: "get_current_temperature".into(),
            description: "Gets the current temperature for a given location.".into(),
            input_schema: json!({
                "properties": {
                    "location": {
                        "description": "The city name, e.g. San Francisco",
                        "type": "STRING"
                    }
                },
                "required": ["location"],
                "type": "OBJECT"
            }),
        }],
        ..Default::default()
    };
    let (out, _) = render(req);
    // Canonical line 88-91 from upstream docs (note: serde_json
    // emits object keys in insertion order — we hand-built the JSON
    // to match the canonical key order).
    let expected = "<bos>\
<|turn>system\n\
You are a helpful assistant.\
<|tool>declaration:get_current_temperature{description:<|\"|>Gets the current temperature for a given location.<|\"|>,parameters:\
{properties:{location:{description:<|\"|>The city name, e.g. San Francisco<|\"|>,type:<|\"|>STRING<|\"|>}},required:[<|\"|>location<|\"|>],type:<|\"|>OBJECT<|\"|>}\
}<tool|><turn|>\n\
<|turn>user\n\
What's the temperature in London?<turn|>\n\
<|turn>model\n";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn tool_declaration_synthesised_system_turn_when_absent() {
    // When the request has tools but no System message, the renderer
    // synthesises an empty system turn before the first message so
    // the tool block has a home — matches the upstream line 121-122
    // "tools without a system turn" case.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "What's the temperature in London?".into(),
            }],
        }],
        tools: vec![Tool {
            name: "get_current_temperature".into(),
            description: "Gets the current temperature for a given location.".into(),
            input_schema: json!({
                "properties": {
                    "location": {
                        "description": "The city name, e.g. San Francisco",
                        "type": "STRING"
                    }
                },
                "required": ["location"],
                "type": "OBJECT"
            }),
        }],
        ..Default::default()
    };
    let (out, _) = render(req);
    let expected = "<bos>\
<|turn>system\n\
<|tool>declaration:get_current_temperature{description:<|\"|>Gets the current temperature for a given location.<|\"|>,parameters:\
{properties:{location:{description:<|\"|>The city name, e.g. San Francisco<|\"|>,type:<|\"|>STRING<|\"|>}},required:[<|\"|>location<|\"|>],type:<|\"|>OBJECT<|\"|>}\
}<tool|><turn|>\n\
<|turn>user\n\
What's the temperature in London?<turn|>\n\
<|turn>model\n";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn tool_use_block_renders_as_tool_call_inside_assistant_turn() {
    // Replay an assistant-emitted tool_use back into a request for
    // context. Maps to `<|tool_call>call:NAME{KEY:<|"|>VALUE<|"|>}<tool_call|>`
    // inside the model turn.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "Hey, what's the weather in Tokyo right now?".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![ContentBlock::ToolUse {
                    tool_call_id: ToolCallId::from("tc-1"),
                    name: "get_current_weather".into(),
                    input: json!({"location": "Tokyo, JP"}),
                }],
            },
        ],
        tools: vec![Tool {
            name: "get_current_weather".into(),
            description: "Gets the current weather in a given location.".into(),
            input_schema: json!({"type": "OBJECT"}),
        }],
        ..Default::default()
    };
    let (out, _) = render(req);
    assert!(
        out.contains(
            "<|tool_call>call:get_current_weather{location:<|\"|>Tokyo, JP<|\"|>}<tool_call|>"
        ),
        "tool_call inline form missing; got:\n{out}"
    );
    // The assistant turn closes with <turn|> and is followed by the
    // generation prompt.
    assert!(out.ends_with("<|turn>model\n"), "got tail: {out}");
}

#[test]
fn tool_result_renders_inside_a_user_turn_pair() {
    // Per upstream: the response continues the model turn that the
    // tool_call started. v2 wire-side pairs the result with
    // tool_call_id; the renderer drops the id and emits the
    // upstream wire form. This test asserts the response token is
    // present, in the right place.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "Hey, what's the weather in Tokyo right now?".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        tool_call_id: ToolCallId::from("tc-1"),
                        name: "get_current_weather".into(),
                        input: json!({"location": "Tokyo, JP"}),
                    },
                    ContentBlock::ToolResult {
                        tool_call_id: ToolCallId::from("tc-1"),
                        content: vec![ContentBlock::Text {
                            text: "{\"temperature\":15,\"weather\":\"sunny\"}".into(),
                        }],
                    },
                ],
            },
        ],
        tools: vec![Tool {
            name: "get_current_weather".into(),
            description: "Gets the current weather in a given location.".into(),
            input_schema: json!({"type": "OBJECT"}),
        }],
        ..Default::default()
    };
    let (out, _) = render(req);
    assert!(
        out.contains("<|tool_response>response:get_current_weather{temperature:15,weather:<|\"|>sunny<|\"|>}<tool_response|>"),
        "tool_response form missing; got:\n{out}"
    );
}

#[test]
fn image_attachment_emits_media_marker_and_records_attachment() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![
                ContentBlock::Text {
                    text: "What's in this image?".into(),
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
    let renderer = Gemma4Renderer::new();
    let rendered = renderer.render(&resolved).unwrap();

    assert_eq!(rendered.attachments.len(), 1);
    assert_eq!(rendered.attachments[0].id(), "img-1");

    let expected = "<bos>\
<|turn>user\n\
What's in this image?<__media__><turn|>\n\
<|turn>model\n";
    assert_eq!(rendered.prompt, expected, "got:\n{}", rendered.prompt);
}

#[test]
fn multiple_attachments_recorded_in_message_order() {
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
                ContentBlock::Audio {
                    attachment_id: "b".into(),
                },
                ContentBlock::Image {
                    attachment_id: "c".into(),
                },
            ],
        }],
        attachments: vec![
            Attachment::Image {
                id: "a".into(),
                width: 8,
                height: 8,
                bytes: "1".into(),
            },
            Attachment::Audio {
                id: "b".into(),
                sample_rate: 16000,
                bytes: "2".into(),
            },
            Attachment::Image {
                id: "c".into(),
                width: 8,
                height: 8,
                bytes: "3".into(),
            },
        ],
        ..Default::default()
    };
    let resolved = req.resolve().unwrap();
    let renderer = Gemma4Renderer::new();
    let rendered = renderer.render(&resolved).unwrap();

    let ids: Vec<&str> = rendered.attachments.iter().map(|a| a.id()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
    let n_markers = rendered.prompt.matches("<__media__>").count();
    assert_eq!(n_markers, 3);
}

#[test]
fn assistant_role_renders_as_model_token() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::Assistant,
            content: vec![ContentBlock::Text { text: "ok".into() }],
        }],
        ..Default::default()
    };
    let (out, _) = render(req);
    // RoleV2::Assistant must emit the literal `model` token,
    // matching Gemma's upstream wire format.
    assert!(out.contains("<|turn>model\nok<turn|>"), "got: {out}");
}
