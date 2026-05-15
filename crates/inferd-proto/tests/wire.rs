//! Wire-format tests for `inferd-proto`. Tier 1 + Tier 5 (F-1) per
//! `docs/test-strategy.md`.

use inferd_proto::{
    read_frame, write_frame, ErrorCode, Message, ProtoError, Request, Response, Role, StopReason,
    Usage, MAX_FRAME_BYTES,
};
use std::io::Cursor;

fn sample_request() -> Request {
    Request {
        id: "abc123".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hello".into(),
        }],
        temperature: Some(0.7),
        top_p: None,
        top_k: Some(40),
        max_tokens: Some(128),
        stream: Some(true),
        image_token_budget: Some(280),
        grammar: String::new(),
    }
}

#[test]
fn request_roundtrip() {
    let req = sample_request();
    let mut buf = Vec::new();
    write_frame(&mut buf, &req).unwrap();
    assert!(buf.ends_with(b"\n"), "frame must end in newline");

    let mut cursor = Cursor::new(buf);
    let parsed: Request = read_frame(&mut cursor).unwrap().expect("frame present");
    assert_eq!(req, parsed);
}

#[test]
fn request_resolve_applies_defaults() {
    let req = Request {
        id: "id".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    };
    let resolved = req.resolve().unwrap();
    assert_eq!(resolved.temperature, 1.0);
    assert_eq!(resolved.top_p, 0.95);
    assert_eq!(resolved.top_k, 64);
    assert_eq!(resolved.max_tokens, 1000);
    assert!(resolved.stream);
    assert!(resolved.image_token_budget.is_none());
}

#[test]
fn empty_messages_rejected() {
    let req = Request {
        id: "id".into(),
        messages: vec![],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    };
    let err = req.resolve().unwrap_err();
    assert!(matches!(err, ProtoError::InvalidRequest(_)));
    assert_eq!(err.to_error_code(), ErrorCode::InvalidRequest);
}

#[test]
fn invalid_image_budget_rejected() {
    let mut req = sample_request();
    req.image_token_budget = Some(999);
    let err = req.resolve().unwrap_err();
    assert!(matches!(err, ProtoError::InvalidRequest(_)));
}

#[test]
fn valid_image_budgets_accepted() {
    for &budget in &[70u32, 140, 280, 560, 1120] {
        let mut req = sample_request();
        req.image_token_budget = Some(budget);
        let resolved = req.resolve().unwrap();
        assert_eq!(resolved.image_token_budget.unwrap().get(), budget);
    }
}

#[test]
fn unknown_fields_ignored_on_parse() {
    // Forward compatibility — additive v1 changes must not break older parsers.
    let json = br#"{"id":"x","messages":[{"role":"user","content":"hi"}],"future_field":42}
"#;
    let mut cursor = Cursor::new(&json[..]);
    let req: Request = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(req.id, "x");
}

#[test]
fn response_done_serialises_with_all_fields() {
    let resp = Response::Done {
        id: "r1".into(),
        content: "the full text".into(),
        usage: Usage {
            prompt_tokens: 12,
            completion_tokens: 7,
        },
        stop_reason: StopReason::End,
        backend: "llamacpp".into(),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &resp).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert!(s.contains(r#""type":"done""#));
    assert!(s.contains(r#""stop_reason":"end""#));
    assert!(s.contains(r#""backend":"llamacpp""#));
    assert!(s.contains(r#""prompt_tokens":12"#));
}

#[test]
fn response_error_carries_code_enum() {
    let resp = Response::Error {
        id: "r1".into(),
        code: ErrorCode::QueueFull,
        message: "queue full".into(),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &resp).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert!(s.contains(r#""code":"queue_full""#));

    let mut cursor = Cursor::new(buf);
    let parsed: Response = read_frame(&mut cursor).unwrap().unwrap();
    if let Response::Error { code, .. } = parsed {
        assert_eq!(code, ErrorCode::QueueFull);
    } else {
        panic!("expected error variant");
    }
}

#[test]
fn response_helpers() {
    let done = Response::Done {
        id: "r2".into(),
        content: String::new(),
        usage: Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
        },
        stop_reason: StopReason::End,
        backend: "mock".into(),
    };
    assert_eq!(done.id(), "r2");
    assert!(done.is_terminal());

    let token = Response::Token {
        id: "r2".into(),
        content: "x".into(),
    };
    assert!(!token.is_terminal());
}

#[test]
fn empty_stream_returns_none() {
    let mut cursor = Cursor::new(Vec::<u8>::new());
    let parsed: Option<Request> = read_frame(&mut cursor).unwrap();
    assert!(parsed.is_none());
}

#[test]
fn trailing_line_without_newline_parses() {
    let json = br#"{"id":"x","messages":[{"role":"user","content":"hi"}]}"#;
    let mut cursor = Cursor::new(&json[..]);
    let req: Request = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(req.id, "x");
}

#[test]
fn multiple_frames_in_one_buffer() {
    let mut buf = Vec::new();
    let r1 = Request {
        id: "1".into(),
        messages: vec![Message {
            role: Role::User,
            content: "a".into(),
        }],
        ..sample_request()
    };
    let r2 = Request {
        id: "2".into(),
        messages: vec![Message {
            role: Role::User,
            content: "b".into(),
        }],
        ..sample_request()
    };
    write_frame(&mut buf, &r1).unwrap();
    write_frame(&mut buf, &r2).unwrap();

    let mut cursor = Cursor::new(buf);
    let p1: Request = read_frame(&mut cursor).unwrap().unwrap();
    let p2: Request = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(p1.id, "1");
    assert_eq!(p2.id, "2");
    let p3: Option<Request> = read_frame(&mut cursor).unwrap();
    assert!(p3.is_none());
}

// THREAT_MODEL F-1: oversized frames are rejected without exhausting heap.
#[test]
fn oversized_frame_rejected() {
    // Construct a single line exceeding the cap. We don't actually allocate
    // 64 MiB of JSON — we use a small cap-equivalent via a custom reader that
    // returns garbage bytes forever and verify the reader stops at the cap.
    use std::io::{BufRead, Read};

    struct Endless;
    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(b'a');
            Ok(buf.len())
        }
    }
    impl BufRead for Endless {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            // Static buffer of non-newline bytes. 8 KiB chunks.
            static CHUNK: [u8; 8192] = [b'a'; 8192];
            Ok(&CHUNK[..])
        }
        fn consume(&mut self, _n: usize) {}
    }

    let mut endless = Endless;
    let err: Result<Option<Request>, _> = read_frame(&mut endless);
    assert!(matches!(err, Err(ProtoError::FrameTooLarge)));
}

#[test]
fn write_frame_rejects_oversize_payload() {
    // Build a Request whose JSON exceeds the cap to confirm we don't write it.
    let huge = "x".repeat(MAX_FRAME_BYTES);
    let req = Request {
        id: "id".into(),
        messages: vec![Message {
            role: Role::User,
            content: huge,
        }],
        ..sample_request()
    };
    let mut buf = Vec::new();
    let err = write_frame(&mut buf, &req).unwrap_err();
    assert!(matches!(err, ProtoError::FrameTooLarge));
}

#[test]
fn malformed_json_returns_decode_error() {
    let bytes = b"{ this is not json }\n";
    let mut cursor = Cursor::new(&bytes[..]);
    let err: Result<Option<Request>, _> = read_frame(&mut cursor);
    let err = err.unwrap_err();
    assert!(matches!(err, ProtoError::Decode(_)));
    assert_eq!(err.to_error_code(), ErrorCode::InvalidRequest);
}
