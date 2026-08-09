//! Integration test for the v2 socket dispatch path (v0.4 / ADR 0021).
//!
//! Pins the contract that the v2 listener — now the single generation
//! surface on the length-prefixed wire:
//!   - parses length-prefixed `RequestV2` JSON frames;
//!   - returns `ResponseV2::Error{InvalidRequest, ...}` for structurally
//!     invalid requests;
//!   - routes raw attachment BLOBs by id to the backend;
//!   - dispatches valid requests through the router to
//!     `Backend::generate_v2`, streaming `TokenEventV2`s back as
//!     length-prefixed `ResponseV2::Frame` / `Done` frames;
//!   - closes the connection after a frame-level decode error.
//!
//! Mock backend advertises v2 + thinking capability; multimodal +
//! tools stay false. Tests that need richer surface configure the
//! mock with explicit token tape.
//!
//! Unix domain sockets are used per ADR 0022 (inbound TCP removed).
//! Platform-specific tests: `echo_pipe.rs` for Windows.

mod common;

#[cfg(unix)]
use common::{
    collect_frames, image_attachment, read_lp_frame, text_request, write_lp_json, write_lp_payload,
    write_request,
};
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
#[cfg(unix)]
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_uds_v2};
#[cfg(unix)]
use inferd_daemon::router::Router;
#[cfg(unix)]
use inferd_engine::mock::{Mock, MockConfig};
#[cfg(unix)]
use inferd_proto::FrameType;
#[cfg(unix)]
use inferd_proto::v2::{
    ContentBlock, ErrorCodeV2, MessageV2, RequestV2, ResponseBlock, ResponseV2, RoleV2,
    StopReasonV2, Tool, ToolChoice,
};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::io::{AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(unix)]
async fn boot_v2_daemon_with_mock(
    mock: Arc<Mock>,
) -> (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let router = Arc::new(Router::new(vec![mock]));

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let idx = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let socket_path = std::env::temp_dir().join(format!(
        "inferd-test-v2_stub-{}-{}.sock",
        std::process::id(),
        idx
    ));
    let _ = std::fs::remove_file(&socket_path);

    let listener = bind_uds(&socket_path, None).await.expect("bind uds");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_uds_v2(listener, router, AcceptContext::default(), shutdown_rx).await;
    });
    (socket_path, shutdown_tx, handle)
}

#[cfg(unix)]
async fn boot_v2_daemon() -> (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    boot_v2_daemon_with_mock(Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["hi".into()],
        ..Default::default()
    })))
    .await
}

#[cfg(unix)]
async fn send_and_read_one(path: &std::path::Path, req: &RequestV2) -> ResponseV2 {
    let mut stream = UnixStream::connect(path).await.expect("connect");
    write_request(&mut stream, req).await;
    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let frames = tokio::time::timeout(Duration::from_secs(5), collect_frames(&mut reader))
        .await
        .expect("read budget exceeded");
    frames.into_iter().next().expect("expected one frame")
}

#[cfg(unix)]
async fn send_and_read_all(path: &std::path::Path, req: &RequestV2) -> Vec<ResponseV2> {
    let mut stream = UnixStream::connect(path).await.expect("connect");
    write_request(&mut stream, req).await;
    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    tokio::time::timeout(Duration::from_secs(5), collect_frames(&mut reader))
        .await
        .expect("read budget exceeded")
}

/// A `tool_choice: required` request whose generation produced no tool
/// call. Build it here rather than in `common` — only these two tests
/// need it.
#[cfg(unix)]
fn required_tool_request(id: &str) -> RequestV2 {
    let mut req = text_request(id, "what time is it?");
    req.tools = vec![Tool {
        name: "get_time".into(),
        description: "current time".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
    }];
    req.tool_choice = Some(ToolChoice::Required);
    req
}

/// `required` + no call must set the flag on the terminal frame.
///
/// This is the ADR 0029 gap made reportable: `required` guarantees the
/// turn cannot *end* without a call, but a model can decline for its
/// whole budget, and the resulting `max_tokens` is indistinguishable
/// from "ran out of room mid-answer". The mock declines by construction
/// (it emits text and stops), which is exactly the shape a real
/// declining model produces.
#[cfg(unix)]
#[tokio::test]
async fn required_tool_choice_with_no_call_sets_tool_choice_unsatisfied() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["I'd rather not.".into()],
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let frames = send_and_read_all(&path, &required_tool_request("v2-tc-unsat")).await;

    match frames.last().expect("a terminal frame") {
        ResponseV2::Done {
            tool_choice_unsatisfied,
            ..
        } => assert!(
            *tool_choice_unsatisfied,
            "required + no tool call must report unsatisfied; got: {frames:#?}"
        ),
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

/// The other branch, and the one that catches a relay that flags
/// unconditionally: same `required` request, but a call *does* arrive.
/// The flag must stay off.
#[cfg(unix)]
#[tokio::test]
async fn required_tool_choice_with_a_call_leaves_the_flag_unset() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["checking".into()],
        emit_tool_use: Some("get_time".into()),
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let frames = send_and_read_all(&path, &required_tool_request("v2-tc-sat")).await;

    match frames.last().expect("a terminal frame") {
        ResponseV2::Done {
            tool_choice_unsatisfied,
            stop_reason,
            ..
        } => {
            assert!(
                !tool_choice_unsatisfied,
                "a call arrived; flag must stay unset: {frames:#?}"
            );
            assert_eq!(*stop_reason, StopReasonV2::ToolUse);
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

/// Without `tool_choice: required` the flag is never set, even when no
/// call arrives — nothing was promised, so there is nothing unsatisfied.
#[cfg(unix)]
#[tokio::test]
async fn absent_tool_choice_never_sets_the_flag() {
    let (path, shutdown, handle) = boot_v2_daemon().await;

    let frames = send_and_read_all(&path, &text_request("v2-tc-none", "hello")).await;

    match frames.last().expect("a terminal frame") {
        ResponseV2::Done {
            tool_choice_unsatisfied,
            ..
        } => assert!(
            !tool_choice_unsatisfied,
            "no tool_choice was sent; flag must stay unset: {frames:#?}"
        ),
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn valid_v2_request_streams_frames_and_terminates_with_done() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["a".into(), "b".into(), "c".into()],
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let frames = send_and_read_all(&path, &text_request("v2-001", "hello")).await;

    // Three Frame{Text} frames plus one Done.
    assert_eq!(frames.len(), 4, "got: {frames:#?}");
    let mut deltas = Vec::new();
    for f in &frames[..3] {
        match f {
            ResponseV2::Frame {
                id,
                block: ResponseBlock::Text { delta },
            } => {
                assert_eq!(id, "v2-001");
                deltas.push(delta.clone());
            }
            other => panic!("expected Frame{{Text}}, got {other:?}"),
        }
    }
    assert_eq!(deltas, vec!["a", "b", "c"]);

    match &frames[3] {
        // Exhaustive on purpose: no `..`, so adding a field to the
        // terminal frame fails this test until someone decides what the
        // baseline value should be. That is how the new
        // `tool_choice_unsatisfied` field got asserted here.
        ResponseV2::Done {
            id,
            usage,
            stop_reason,
            backend,
            tool_choice_unsatisfied,
        } => {
            assert_eq!(id, "v2-001");
            assert_eq!(*stop_reason, StopReasonV2::EndTurn);
            assert_eq!(usage.output_tokens, 3);
            assert_eq!(backend, "mock");
            assert!(!tool_choice_unsatisfied, "this request sent no tool_choice");
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn valid_multimodal_v2_request_dispatches_to_backend() {
    // Mock doesn't actually ingest images, but the daemon's v2 path
    // should route the structurally-valid request (image content block
    // + matching attachment BLOB) through Backend::generate_v2 without
    // complaining at the gateway layer about attachment kinds.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["seen".into()],
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let req = RequestV2 {
        id: "v2-002".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![
                ContentBlock::Text {
                    text: "what's in this?".into(),
                },
                ContentBlock::Image {
                    attachment_id: "img-1".into(),
                },
            ],
        }],
        // 2x2 RGB = 12 bytes; routed as a BLOB frame by id.
        attachments: vec![image_attachment("img-1", 2, 2, vec![7u8; 12])],
        ..Default::default()
    };

    let frames = send_and_read_all(&path, &req).await;
    let last = frames.last().expect("zero frames");
    match last {
        ResponseV2::Done { id, .. } => assert_eq!(id, "v2-002"),
        other => panic!("expected terminal Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_v2_request_dangling_attachment_returns_invalid_request() {
    let (path, shutdown, handle) = boot_v2_daemon().await;

    let req = RequestV2 {
        id: "v2-bad-1".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Image {
                attachment_id: "missing".into(),
            }],
        }],
        ..Default::default()
    };

    let resp = send_and_read_one(&path, &req).await;
    match resp {
        ResponseV2::Error { id, code, message } => {
            assert_eq!(id, "v2-bad-1");
            assert_eq!(code, ErrorCodeV2::InvalidRequest);
            assert!(
                message.contains("missing"),
                "message should name the dangling id; got: {message}"
            );
        }
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_v2_request_empty_messages_returns_invalid_request() {
    let (path, shutdown, handle) = boot_v2_daemon().await;

    let req = RequestV2 {
        id: "v2-bad-2".into(),
        messages: vec![],
        ..Default::default()
    };

    let resp = send_and_read_one(&path, &req).await;
    match resp {
        ResponseV2::Error { id, code, .. } => {
            assert_eq!(id, "v2-bad-2");
            assert_eq!(code, ErrorCodeV2::InvalidRequest);
        }
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_json_on_v2_returns_invalid_request_then_closes() {
    let (path, shutdown, handle) = boot_v2_daemon().await;

    let mut stream = UnixStream::connect(&path).await.expect("connect");
    // A well-framed length-prefixed JSON frame whose payload isn't valid
    // JSON: the daemon must emit an InvalidRequest error frame and close.
    write_lp_payload(
        &mut stream,
        FrameType::Json as u8,
        b"{this is not valid json",
    )
    .await;
    stream.flush().await.expect("flush");

    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let (type_byte, payload) = read_lp_frame(&mut reader)
        .await
        .expect("daemon must emit a v2 error frame");
    assert_eq!(type_byte, FrameType::Json as u8);
    let resp: ResponseV2 =
        serde_json::from_slice(&payload).expect("daemon must emit valid v2 error frame");
    match resp {
        ResponseV2::Error {
            code: ErrorCodeV2::InvalidRequest,
            ..
        } => {}
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    // Per ADR 0021: connection is closed after a frame-level decode error.
    let after = read_lp_frame(&mut reader).await;
    assert!(after.is_none(), "expected EOF after bad-json error frame");

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn multiple_v2_requests_on_one_connection_each_terminate_independently() {
    // After a successful request, the daemon should keep the connection
    // open and process the next frame.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["once".into()],
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let mut stream = UnixStream::connect(&path).await.expect("connect");
    for i in 0..3 {
        write_request(&mut stream, &text_request(&format!("v2-loop-{i}"), "hi")).await;
    }

    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut done_ids = Vec::new();
    // Each request emits 1 Frame{Text} + 1 Done = 2 frames; 3 requests
    // = 6 frames total.
    for _ in 0..6 {
        let (_t, payload) =
            tokio::time::timeout(Duration::from_secs(5), read_lp_frame(&mut reader))
                .await
                .expect("read budget")
                .expect("EOF before all frames received");
        let resp: ResponseV2 = serde_json::from_slice(&payload).expect("decode");
        if let ResponseV2::Done { id, .. } = resp {
            done_ids.push(id);
        }
    }
    assert_eq!(
        done_ids,
        vec![
            "v2-loop-0".to_string(),
            "v2-loop-1".to_string(),
            "v2-loop-2".to_string()
        ]
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn pre_stream_unavailable_returns_backend_unavailable() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["never".into()],
        pre_stream_error: Some(inferd_engine::mock::MockError::Unavailable),
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let resp = send_and_read_one(&path, &text_request("v2-fail", "hi")).await;
    match resp {
        ResponseV2::Error { id, code, .. } => {
            assert_eq!(id, "v2-fail");
            assert_eq!(code, ErrorCodeV2::BackendUnavailable);
        }
        other => panic!("expected Error{{BackendUnavailable,..}}, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn mid_stream_drop_emits_backend_unavailable_terminal() {
    // Mock yields 2 tokens then drops without Done. The daemon must
    // synthesise a terminal Error{BackendUnavailable,...} so the client
    // doesn't hang.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["a".into(), "b".into(), "c".into()],
        mid_stream_drop_after: Some(2),
        ..Default::default()
    }));
    let (path, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let frames = send_and_read_all(&path, &text_request("v2-mid", "hi")).await;
    // 2 Frame{Text} + 1 Error = 3.
    assert_eq!(frames.len(), 3, "got: {frames:#?}");
    match frames.last().unwrap() {
        ResponseV2::Error { id, code, .. } => {
            assert_eq!(id, "v2-mid");
            assert_eq!(*code, ErrorCodeV2::BackendUnavailable);
        }
        other => panic!("expected terminal Error, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

/// W2 (v0.4-validation §"Gate 2"): a request whose `wire_version` the
/// daemon doesn't speak must get a single
/// `Error{WireVersionUnsupported}` frame and then the connection must
/// close — the daemon must NOT parse the body or resync (ADR 0021).
#[cfg(unix)]
#[tokio::test]
async fn unsupported_wire_version_errors_and_closes() {
    let (path, shutdown, handle) = boot_v2_daemon().await;

    let mut stream = UnixStream::connect(&path).await.expect("connect");
    // Build a request with a bogus wire_version. `write_request` would
    // overwrite it with WIRE_VERSION, so frame it directly.
    let mut req = text_request("v2-badver", "hi");
    req.wire_version = 99;
    write_lp_json(&mut stream, &req).await;
    stream.flush().await.expect("flush");

    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let (type_byte, payload) =
        tokio::time::timeout(Duration::from_secs(5), read_lp_frame(&mut reader))
            .await
            .expect("read budget exceeded")
            .expect("daemon must emit a v2 error frame");
    assert_eq!(type_byte, FrameType::Json as u8);
    let resp: ResponseV2 = serde_json::from_slice(&payload).expect("decode v2 error frame");
    match resp {
        ResponseV2::Error { id, code, message } => {
            assert_eq!(id, "v2-badver");
            assert_eq!(code, ErrorCodeV2::WireVersionUnsupported);
            assert!(
                message.contains("wire_version"),
                "message should explain the mismatch; got: {message}"
            );
        }
        other => panic!("expected Error{{WireVersionUnsupported,..}}, got {other:?}"),
    }

    // Connection must close after the version error — no resync.
    let after = tokio::time::timeout(Duration::from_secs(2), read_lp_frame(&mut reader))
        .await
        .expect("EOF wait exceeded");
    assert!(after.is_none(), "expected EOF after wire_version error");

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
