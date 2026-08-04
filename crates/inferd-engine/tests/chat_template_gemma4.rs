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

use inferd_engine::llamacpp::{ChatRenderer, Gemma4Renderer};
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
fn thinking_injects_think_token_into_existing_system_turn() {
    // thinking=true with a system message: `<|think|>` leads the system
    // turn, before the system text (GA activation, ADR 0013).
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
                content: vec![ContentBlock::Text { text: "hi".into() }],
            },
        ],
        thinking: Some(true),
        ..Default::default()
    };
    let (out, _) = render(req);
    let expected = "<bos>\
<|turn>system\n\
<|think|>You are a helpful assistant.<turn|>\n\
<|turn>user\n\
hi<turn|>\n\
<|turn>model\n";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn thinking_synthesizes_system_turn_when_none_present() {
    // thinking=true with NO system message: synthesise an empty system
    // turn carrying just `<|think|>` before the first message.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        thinking: Some(true),
        ..Default::default()
    };
    let (out, _) = render(req);
    let expected = "<bos>\
<|turn>system\n\
<|think|><turn|>\n\
<|turn>user\n\
hi<turn|>\n\
<|turn>model\n";
    assert_eq!(out, expected, "got:\n{out}");
}

#[test]
fn no_thinking_is_unchanged() {
    // thinking unset → no `<|think|>` anywhere (behaviour-preserving).
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        ..Default::default()
    };
    let (out, _) = render(req);
    assert!(
        !out.contains("<|think|>"),
        "no thinking requested; got:\n{out}"
    );
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
fn tool_result_pairs_via_tool_call_id_when_multiple_tools_in_scope() {
    // Phase 4B: ToolResult must resolve its tool name from the
    // tool_call_id of the matching ToolUse, even when tools[] has
    // more than one entry (so the single-tool fallback can't help).
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "Weather and time?".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        tool_call_id: ToolCallId::from("tc-w"),
                        name: "get_weather".into(),
                        input: json!({"location": "Tokyo"}),
                    },
                    ContentBlock::ToolUse {
                        tool_call_id: ToolCallId::from("tc-t"),
                        name: "get_time".into(),
                        input: json!({"tz": "Asia/Tokyo"}),
                    },
                ],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_call_id: ToolCallId::from("tc-t"),
                        content: vec![ContentBlock::Text {
                            text: "{\"hh\":15}".into(),
                        }],
                    },
                    ContentBlock::ToolResult {
                        tool_call_id: ToolCallId::from("tc-w"),
                        content: vec![ContentBlock::Text {
                            text: "{\"temp\":20}".into(),
                        }],
                    },
                ],
            },
        ],
        tools: vec![
            Tool {
                name: "get_weather".into(),
                description: "Weather.".into(),
                input_schema: json!({"type": "OBJECT"}),
            },
            Tool {
                name: "get_time".into(),
                description: "Time.".into(),
                input_schema: json!({"type": "OBJECT"}),
            },
        ],
        ..Default::default()
    };
    let (out, _) = render(req);
    // Each result must pair to its own tool name via tool_call_id —
    // not be misrouted by ordering.
    assert!(
        out.contains("<|tool_response>response:get_time{hh:15}<tool_response|>"),
        "get_time response missing or misrouted; got:\n{out}"
    );
    assert!(
        out.contains("<|tool_response>response:get_weather{temp:20}<tool_response|>"),
        "get_weather response missing or misrouted; got:\n{out}"
    );
}

#[test]
fn tool_result_in_user_turn_round_trip_after_assistant_tool_use() {
    // Phase 4B end-to-end shape: model emits a tool_call in an
    // assistant turn → consumer constructs a follow-up request that
    // appends a user-role message containing the matching ToolResult.
    // The renderer must produce a valid prompt where the original
    // tool_call survives in the assistant turn AND the response is
    // emitted (in whichever turn the consumer placed it) using the
    // correct tool name resolved from tool_call_id.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text {
                    text: "Weather in Tokyo?".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::Assistant,
                content: vec![ContentBlock::ToolUse {
                    tool_call_id: ToolCallId::from("tc-1"),
                    name: "get_weather".into(),
                    input: json!({"location": "Tokyo"}),
                }],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::ToolResult {
                    tool_call_id: ToolCallId::from("tc-1"),
                    content: vec![ContentBlock::Text {
                        text: "{\"temp\":20}".into(),
                    }],
                }],
            },
        ],
        tools: vec![
            Tool {
                name: "get_weather".into(),
                description: "Weather.".into(),
                input_schema: json!({"type": "OBJECT"}),
            },
            Tool {
                name: "unrelated".into(),
                description: "Other.".into(),
                input_schema: json!({"type": "OBJECT"}),
            },
        ],
        ..Default::default()
    };
    let (out, _) = render(req);
    // Original tool_call survives in the assistant turn.
    assert!(
        out.contains(
            "<|turn>model\n<|tool_call>call:get_weather{location:<|\"|>Tokyo<|\"|>}<tool_call|><turn|>"
        ),
        "tool_call missing in assistant turn; got:\n{out}"
    );
    // Response paired to tool_call_id, not the wrong sibling tool.
    assert!(
        out.contains("<|tool_response>response:get_weather{temp:20}<tool_response|>"),
        "tool_response paired to wrong tool name or missing; got:\n{out}"
    );
}

#[test]
fn tool_result_with_unknown_tool_call_id_falls_through_to_raw_content() {
    // If a ToolResult arrives without a matching ToolUse and tools[]
    // is ambiguous, the renderer falls back to raw content rather
    // than guessing.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::ToolResult {
                tool_call_id: ToolCallId::from("does-not-exist"),
                content: vec![ContentBlock::Text {
                    text: "freeform output".into(),
                }],
            }],
        }],
        tools: vec![
            Tool {
                name: "a".into(),
                description: ".".into(),
                input_schema: json!({"type": "OBJECT"}),
            },
            Tool {
                name: "b".into(),
                description: ".".into(),
                input_schema: json!({"type": "OBJECT"}),
            },
        ],
        ..Default::default()
    };
    let (out, _) = render(req);
    // No `response:NAME{...}` wrapper — just raw content between the
    // sentinels.
    assert!(
        out.contains("<|tool_response>freeform output<tool_response|>"),
        "raw fallback missing; got:\n{out}"
    );
    assert!(
        !out.contains("response:"),
        "should not invent a tool name; got:\n{out}"
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
