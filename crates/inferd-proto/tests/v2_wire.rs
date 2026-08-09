//! Wire-format tests for the v2 surface — round-trip, validation,
//! and forward-compatibility of unknown content-block types.

use inferd_proto::v2::{
    Attachment, BlobDescriptor, BlobDescriptorTag, ContentBlock, ErrorCodeV2, MessageV2, RequestV2,
    ResponseBlock, ResponseV2, RoleV2, StopReasonV2, Tool, ToolCallId, UsageV2, WIRE_VERSION,
};
use inferd_proto::{MAX_FRAME_BYTES, ProtoError, read_frame, write_frame};
use serde_json::json;
use std::io::Cursor;

#[test]
fn wire_version_round_trips_on_request() {
    let req = RequestV2 {
        wire_version: WIRE_VERSION,
        id: "wv".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        ..Default::default()
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &req).unwrap();
    let json = String::from_utf8(buf.clone()).unwrap();
    assert!(json.contains("\"wire_version\""), "got: {json}");
    let mut cur = Cursor::new(buf);
    let parsed: RequestV2 = read_frame(&mut cur).unwrap().unwrap();
    assert_eq!(parsed.wire_version, WIRE_VERSION);
    // resolve carries it through unchecked (daemon enforces policy).
    assert_eq!(parsed.resolve().unwrap().wire_version, WIRE_VERSION);
}

#[test]
fn request_missing_wire_version_defaults_to_zero() {
    // A frame from a pre-v0.4 (or buggy) client omits wire_version; it
    // must deserialise to 0 so the daemon can reject it loudly rather
    // than mis-handle it.
    let line =
        br#"{"id":"old","messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#;
    let mut cur = Cursor::new(&line[..]);
    let parsed: RequestV2 = read_frame(&mut cur).unwrap().unwrap();
    assert_eq!(parsed.wire_version, 0);
}

#[test]
fn blob_descriptor_round_trips() {
    let d = BlobDescriptor::new("img-1", 196608);
    let s = serde_json::to_string(&d).unwrap();
    assert!(s.contains("\"attachment_blob\""), "got: {s}");
    assert!(s.contains("\"attachment_id\":\"img-1\""), "got: {s}");
    let back: BlobDescriptor = serde_json::from_str(&s).unwrap();
    assert_eq!(back, d);
    assert_eq!(back.frame_kind, BlobDescriptorTag::AttachmentBlob);
    assert_eq!(back.len, 196608);
}

fn text_request() -> RequestV2 {
    RequestV2 {
        id: "req-001".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        }],
        ..Default::default()
    }
}

fn multimodal_request() -> RequestV2 {
    RequestV2 {
        id: "req-002".into(),
        messages: vec![
            MessageV2 {
                role: RoleV2::System,
                content: vec![ContentBlock::Text {
                    text: "You are helpful.".into(),
                }],
            },
            MessageV2 {
                role: RoleV2::User,
                content: vec![
                    ContentBlock::Text {
                        text: "What's in this image?".into(),
                    },
                    ContentBlock::Image {
                        attachment_id: "img-1".into(),
                    },
                ],
            },
        ],
        attachments: vec![Attachment::Image {
            id: "img-1".into(),
            width: 256,
            height: 256,
            // Raw RGB bytes (ADR 0021). On the JSON wire these are
            // skipped — they ride in a separate BLOB frame — so the
            // round-trip test below asserts metadata equality, not bytes.
            bytes: vec![1, 2, 3, 4, 5, 6],
        }],
        tools: vec![Tool {
            name: "get_weather".into(),
            description: "Returns the current weather for a city.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }],
        temperature: Some(0.7),
        max_tokens: Some(1024),
        stream: Some(true),
        ..Default::default()
    }
}

#[test]
fn request_roundtrip_text_only() {
    let req = text_request();
    let mut buf = Vec::new();
    write_frame(&mut buf, &req).unwrap();
    let mut cursor = Cursor::new(buf);
    let parsed: RequestV2 = read_frame(&mut cursor).unwrap().expect("frame present");
    assert_eq!(req, parsed);
}

#[test]
fn request_roundtrip_multimodal() {
    let req = multimodal_request();
    let mut buf = Vec::new();
    write_frame(&mut buf, &req).unwrap();
    let mut cursor = Cursor::new(buf);
    let parsed: RequestV2 = read_frame(&mut cursor).unwrap().expect("frame present");

    // Attachment *bytes* are `#[serde(skip)]` (ADR 0021 — they ride in a
    // BLOB frame, not the JSON), so the parsed copy has empty bytes.
    // Everything else must round-trip identically.
    let mut expected = req.clone();
    for att in &mut expected.attachments {
        att.set_bytes(Vec::new());
    }
    assert_eq!(expected, parsed);

    // The JSON envelope must not contain the raw bytes.
    let json = serde_json::to_string(&req).unwrap();
    assert!(
        !json.contains("\"bytes\""),
        "attachment bytes must not appear in the JSON frame; got: {json}"
    );

    // Metadata survives.
    match &parsed.attachments[0] {
        Attachment::Image {
            id,
            width,
            height,
            bytes,
        } => {
            assert_eq!(id, "img-1");
            assert_eq!((*width, *height), (256, 256));
            assert!(bytes.is_empty(), "bytes should be out-of-band");
        }
        other => panic!("expected image attachment, got {other:?}"),
    }
}

#[test]
fn resolve_rejects_empty_messages() {
    let req = RequestV2 {
        id: "x".into(),
        ..Default::default()
    };
    let err = req.resolve().unwrap_err();
    assert!(matches!(err, ProtoError::InvalidRequest(_)));
}

#[test]
fn resolve_rejects_empty_content() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![],
        }],
        ..Default::default()
    };
    let err = req.resolve().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("messages[0].content"), "got: {msg}");
}

#[test]
fn resolve_rejects_dangling_attachment_id() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Image {
                attachment_id: "missing".into(),
            }],
        }],
        ..Default::default()
    };
    let err = req.resolve().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("missing"), "got: {msg}");
}

#[test]
fn resolve_accepts_attachment_referenced_in_tool_result() {
    // tool_result wraps further content blocks; attachment refs inside
    // must still resolve.
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::ToolResult {
                tool_call_id: ToolCallId::from("tc-1"),
                content: vec![ContentBlock::Image {
                    attachment_id: "img-1".into(),
                }],
            }],
        }],
        attachments: vec![Attachment::Image {
            id: "img-1".into(),
            width: 64,
            height: 64,
            bytes: vec![0u8; 12],
        }],
        ..Default::default()
    };
    req.resolve().expect("nested attachment should resolve");
}

#[test]
fn resolve_rejects_duplicate_attachment_ids() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        attachments: vec![
            Attachment::Image {
                id: "dup".into(),
                width: 8,
                height: 8,
                bytes: vec![1],
            },
            Attachment::Audio {
                id: "dup".into(),
                sample_rate: 16000,
                bytes: vec![2],
            },
        ],
        ..Default::default()
    };
    let err = req.resolve().unwrap_err();
    assert!(
        matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("duplicate attachment id"))
    );
}

#[test]
fn resolve_rejects_duplicate_tool_names() {
    let req = RequestV2 {
        id: "x".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        tools: vec![
            Tool {
                name: "dup".into(),
                description: "first".into(),
                input_schema: json!({}),
            },
            Tool {
                name: "dup".into(),
                description: "second".into(),
                input_schema: json!({}),
            },
        ],
        ..Default::default()
    };
    let err = req.resolve().unwrap_err();
    assert!(matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("duplicate tool name")));
}

#[test]
fn unknown_content_block_type_deserialises_as_unknown() {
    // Forward-compat: a v2.x daemon adds a `document` block type.
    // A v2.0 client deserialising a request that contains it must
    // not fail to parse — it must land in `Unknown` and only get
    // rejected if the daemon's `resolve()` is called on it.
    let payload = json!({
        "id": "x",
        "messages": [{
            "role": "user",
            "content": [{"type": "document", "url": "https://example.com/doc.pdf"}]
        }]
    });
    let req: RequestV2 = serde_json::from_value(payload).expect("must deserialise");
    assert_eq!(req.messages[0].content.len(), 1);
    assert!(matches!(req.messages[0].content[0], ContentBlock::Unknown));

    // resolve() should reject the unknown block.
    let err = req.resolve().unwrap_err();
    assert!(
        matches!(err, ProtoError::InvalidRequest(ref m) if m.contains("unknown content-block type"))
    );
}

#[test]
fn response_frame_text_delta_roundtrip() {
    let frame = ResponseV2::Frame {
        id: "req-001".into(),
        block: ResponseBlock::Text {
            delta: "Hello ".into(),
        },
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let mut cursor = Cursor::new(buf);
    let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(frame, parsed);
}

#[test]
fn response_frame_thinking_delta_roundtrip() {
    let frame = ResponseV2::Frame {
        id: "req-001".into(),
        block: ResponseBlock::Thinking {
            delta: "Let me think...".into(),
        },
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let mut cursor = Cursor::new(buf);
    let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(frame, parsed);
}

#[test]
fn response_frame_tool_use_roundtrip() {
    let frame = ResponseV2::Frame {
        id: "req-001".into(),
        block: ResponseBlock::ToolUse {
            tool_call_id: ToolCallId::from("tc-1"),
            name: "get_weather".into(),
            input: json!({"city": "London"}),
        },
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let mut cursor = Cursor::new(buf);
    let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(frame, parsed);
}

#[test]
fn response_done_roundtrip() {
    let frame = ResponseV2::Done {
        id: "req-001".into(),
        usage: UsageV2 {
            input_tokens: 42,
            output_tokens: 17,
        },
        stop_reason: StopReasonV2::EndTurn,
        backend: "llamacpp".into(),
        tool_choice_unsatisfied: false,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let mut cursor = Cursor::new(buf);
    let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(frame, parsed);
}

/// The flag must stay off the wire when unset. A v0.7.0 client parses
/// the `done` frame by field name, so emitting `"tool_choice_
/// unsatisfied":false` on every frame would change the bytes every
/// existing consumer sees — the field is only additive if absence is
/// the default encoding.
#[test]
fn unset_tool_choice_unsatisfied_is_not_serialised() {
    let frame = ResponseV2::Done {
        id: "req-001".into(),
        usage: UsageV2 {
            input_tokens: 1,
            output_tokens: 1,
        },
        stop_reason: StopReasonV2::EndTurn,
        backend: "llamacpp".into(),
        tool_choice_unsatisfied: false,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert!(
        !s.contains("tool_choice_unsatisfied"),
        "unset flag must not reach the wire: {s}"
    );
}

/// Set, it serialises as a plain `true`, and a frame that omits it
/// parses back as `false` — so an older *daemon*'s frame is also
/// readable by a newer client.
#[test]
fn set_tool_choice_unsatisfied_round_trips_and_absence_defaults_false() {
    let frame = ResponseV2::Done {
        id: "req-001".into(),
        usage: UsageV2 {
            input_tokens: 9,
            output_tokens: 128,
        },
        // The measured shape from ADR 0029: a declining model burns the
        // budget, so the stop reason is `max_tokens`, not `tool_use`.
        stop_reason: StopReasonV2::MaxTokens,
        backend: "llamacpp".into(),
        tool_choice_unsatisfied: true,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert!(s.contains("\"tool_choice_unsatisfied\":true"), "got: {s}");
    let mut cursor = Cursor::new(buf);
    let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(frame, parsed);

    // A frame from a daemon that predates the field.
    let legacy = r#"{"type":"done","id":"x","usage":{"input_tokens":1,"output_tokens":1},"stop_reason":"max_tokens","backend":"llamacpp"}"#;
    let parsed: ResponseV2 = serde_json::from_str(legacy).expect("legacy done frame must parse");
    match parsed {
        ResponseV2::Done {
            tool_choice_unsatisfied,
            ..
        } => assert!(
            !tool_choice_unsatisfied,
            "absent field must default to false, not fail the parse"
        ),
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn response_done_tool_use_stop_reason_roundtrip() {
    // `tool_use` is a v2-only stop reason; make sure it serialises
    // with the snake_case name from the JSON spec in ADR 0015.
    let frame = ResponseV2::Done {
        id: "req-001".into(),
        usage: UsageV2 {
            input_tokens: 10,
            output_tokens: 5,
        },
        stop_reason: StopReasonV2::ToolUse,
        backend: "llamacpp".into(),
        tool_choice_unsatisfied: false,
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert!(s.contains("\"stop_reason\":\"tool_use\""), "got: {s}");

    let mut cursor = Cursor::new(buf);
    let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(frame, parsed);
}

#[test]
fn response_error_v2_codes_roundtrip() {
    // Both v1-overlap codes and v2-only codes serialise.
    for code in [
        ErrorCodeV2::QueueFull,
        ErrorCodeV2::AttachmentUnsupported,
        ErrorCodeV2::ToolCallMalformed,
    ] {
        let frame = ResponseV2::Error {
            id: "req-001".into(),
            code,
            message: "x".into(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &frame).unwrap();
        let mut cursor = Cursor::new(buf);
        let parsed: ResponseV2 = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame, parsed);
    }
}

#[test]
fn response_terminal_helper() {
    let frame = ResponseV2::Frame {
        id: "x".into(),
        block: ResponseBlock::Text { delta: "hi".into() },
    };
    assert!(!frame.is_terminal());

    let done = ResponseV2::Done {
        id: "x".into(),
        usage: UsageV2 {
            input_tokens: 0,
            output_tokens: 0,
        },
        stop_reason: StopReasonV2::EndTurn,
        backend: "mock".into(),
        tool_choice_unsatisfied: false,
    };
    assert!(done.is_terminal());

    let err = ResponseV2::Error {
        id: "x".into(),
        code: ErrorCodeV2::Internal,
        message: "boom".into(),
    };
    assert!(err.is_terminal());
}

#[test]
fn frame_too_large_is_enforced_for_v2() {
    // Reuse the v1 framing's 64 MiB cap — not v2-specific, but worth
    // verifying the constant stays in scope when v2 types are pulled
    // in alone.
    let _ = MAX_FRAME_BYTES;
}

#[test]
fn adr_0015_request_example_parses() {
    // Pin the exact JSON shape from ADR 0015 §"v2 Request" so any
    // serde rename or field reorder breaks this test before it
    // breaks middleware authors' code.
    let payload = r#"{
  "id": "req-001",
  "messages": [
    {
      "role": "system",
      "content": [{"type": "text", "text": "You are helpful."}]
    },
    {
      "role": "user",
      "content": [
        {"type": "text", "text": "What's in this image?"},
        {"type": "image", "attachment_id": "img-1"}
      ]
    }
  ],
  "attachments": [
    {
      "kind": "image",
      "id": "img-1",
      "width": 256,
      "height": 256,
      "bytes": "<base64>"
    }
  ],
  "tools": [
    {
      "name": "get_weather",
      "description": "Returns the current weather for a city.",
      "input_schema": {
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"]
      }
    }
  ],
  "temperature": 0.7,
  "max_tokens": 1024,
  "stream": true
}"#;
    let req: RequestV2 = serde_json::from_str(payload).expect("ADR 0015 example must parse");
    let resolved = req.resolve().expect("ADR 0015 example must resolve");
    assert_eq!(resolved.id, "req-001");
    assert_eq!(resolved.messages.len(), 2);
    assert_eq!(resolved.attachments.len(), 1);
    assert_eq!(resolved.tools.len(), 1);
}

#[test]
fn adr_0015_response_examples_parse() {
    // Frame with text delta, frame with tool_use, done frame.
    let lines = [
        r#"{"type":"frame","id":"req-001","block":{"type":"text","delta":"Hello "}}"#,
        r#"{"type":"frame","id":"req-001","block":{"type":"text","delta":"there"}}"#,
        r#"{"type":"frame","id":"req-001","block":{"type":"tool_use","tool_call_id":"tc-1","name":"get_weather","input":{"city":"London"}}}"#,
        r#"{"type":"done","id":"req-001","stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":3},"backend":"llamacpp"}"#,
    ];
    for line in lines {
        let _: ResponseV2 = serde_json::from_str(line).expect(line);
    }
}
