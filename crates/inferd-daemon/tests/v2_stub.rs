//! Integration test for the Phase 2A v2 socket dispatch path.
//!
//! Pins the contract that the v2 listener:
//!   - accepts NDJSON connections on its own socket (separate from v1);
//!   - parses `RequestV2` frames;
//!   - returns `ResponseV2::Error{InvalidRequest, ...}` for structurally
//!     invalid requests;
//!   - dispatches valid requests through the router to
//!     `Backend::generate_v2`, streams the resulting `TokenEventV2`s
//!     back as `ResponseV2::Frame` / `Done` frames.
//!
//! Mock backend advertises v2 + thinking capability; multimodal +
//! tools stay false. Tests that need richer surface configure the
//! mock with explicit token tape.

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_tcp_v2};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::v2::{
    Attachment, ContentBlock, ErrorCodeV2, MessageV2, RequestV2, ResponseBlock, ResponseV2, RoleV2,
    StopReasonV2,
};
use inferd_proto::write_frame;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

async fn boot_v2_daemon_with_mock(
    mock: Arc<Mock>,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let router = Arc::new(Router::new(vec![mock]));
    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();
    let ctx = AcceptContext {
        expected_api_key: None,
        admission: None,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp_v2(listener, router, ctx, shutdown_rx).await;
    });
    (addr, shutdown_tx, handle)
}

async fn boot_v2_daemon() -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    boot_v2_daemon_with_mock(Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["hi".into()],
        ..Default::default()
    })))
    .await
}

async fn send_and_read_one(addr: &str, req: &RequestV2) -> ResponseV2 {
    let mut stream = TcpStream::connect(addr).await.expect("connect");

    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, req).expect("encode v2 request");
    stream.write_all(&buf).await.expect("write request");
    stream.flush().await.expect("flush");

    let (read_half, _write_half) = stream.into_split();
    let mut reader = BufReader::with_capacity(8 * 1024, read_half);
    let mut line = Vec::with_capacity(512);
    let n = tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut line))
        .await
        .expect("read budget exceeded")
        .expect("read frame");
    assert!(n > 0, "expected one response frame, got EOF");
    serde_json::from_slice(&line).expect("decode v2 response frame")
}

/// Send one request and collect every response frame until a Done /
/// Error terminal frame.
async fn send_and_read_all(addr: &str, req: &RequestV2) -> Vec<ResponseV2> {
    let mut stream = TcpStream::connect(addr).await.expect("connect");

    let mut buf = Vec::with_capacity(512);
    write_frame(&mut buf, req).expect("encode v2 request");
    stream.write_all(&buf).await.expect("write request");
    stream.flush().await.expect("flush");

    let (read_half, _write_half) = stream.into_split();
    let mut reader = BufReader::with_capacity(8 * 1024, read_half);
    let mut frames = Vec::new();
    let mut line = Vec::with_capacity(512);
    loop {
        line.clear();
        let n = tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut line))
            .await
            .expect("read budget exceeded")
            .expect("read frame");
        if n == 0 {
            break;
        }
        let resp: ResponseV2 = serde_json::from_slice(&line).expect("decode v2 response frame");
        let terminal = resp.is_terminal();
        frames.push(resp);
        if terminal {
            break;
        }
    }
    frames
}

#[tokio::test]
async fn valid_v2_request_streams_frames_and_terminates_with_done() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["a".into(), "b".into(), "c".into()],
        ..Default::default()
    }));
    let (addr, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let req = RequestV2 {
        id: "v2-001".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        }],
        ..Default::default()
    };

    let frames = send_and_read_all(&addr, &req).await;

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
        ResponseV2::Done {
            id,
            usage,
            stop_reason,
            backend,
        } => {
            assert_eq!(id, "v2-001");
            assert_eq!(*stop_reason, StopReasonV2::EndTurn);
            assert_eq!(usage.output_tokens, 3);
            assert_eq!(backend, "mock");
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn valid_multimodal_v2_request_dispatches_to_backend() {
    // Mock doesn't actually ingest images, but the daemon's v2 path
    // should route the structurally-valid request through the
    // Backend::generate_v2 method without complaining at the gateway
    // layer about attachment kinds. Multimodal-payload-rejection is
    // a backend-capability concern (Phase 3); for now Mock just
    // streams its tape regardless of attachments[].
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["seen".into()],
        ..Default::default()
    }));
    let (addr, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

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
        attachments: vec![Attachment::Image {
            id: "img-1".into(),
            width: 64,
            height: 64,
            bytes: "base64data".into(),
        }],
        ..Default::default()
    };

    let frames = send_and_read_all(&addr, &req).await;
    let last = frames.last().expect("zero frames");
    match last {
        ResponseV2::Done { id, .. } => assert_eq!(id, "v2-002"),
        other => panic!("expected terminal Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn invalid_v2_request_dangling_attachment_returns_invalid_request() {
    let (addr, shutdown, handle) = boot_v2_daemon().await;

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

    let resp = send_and_read_one(&addr, &req).await;
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

#[tokio::test]
async fn invalid_v2_request_empty_messages_returns_invalid_request() {
    let (addr, shutdown, handle) = boot_v2_daemon().await;

    let req = RequestV2 {
        id: "v2-bad-2".into(),
        messages: vec![],
        ..Default::default()
    };

    let resp = send_and_read_one(&addr, &req).await;
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

#[tokio::test]
async fn malformed_json_on_v2_returns_invalid_request_then_closes() {
    let (addr, shutdown, handle) = boot_v2_daemon().await;

    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    stream
        .write_all(b"{this is not valid json\n")
        .await
        .expect("write");
    stream.flush().await.expect("flush");

    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::with_capacity(4096, read_half);
    let mut line = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut line))
        .await
        .expect("read budget exceeded")
        .expect("read");
    let resp: ResponseV2 =
        serde_json::from_slice(&line).expect("daemon must emit valid v2 error frame");
    match resp {
        ResponseV2::Error {
            code: ErrorCodeV2::InvalidRequest,
            ..
        } => {}
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    // Per docs: connection is closed after a frame-level decode error.
    let mut after = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(2), reader.read_until(b'\n', &mut after))
        .await
        .expect("EOF wait")
        .expect("read EOF");
    assert_eq!(n, 0, "expected EOF after bad-json error frame");

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn multiple_v2_requests_on_one_connection_each_terminate_independently() {
    // After a successful request, the daemon should keep the
    // connection open and process the next frame.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["once".into()],
        ..Default::default()
    }));
    let (addr, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let mut stream = TcpStream::connect(&addr).await.expect("connect");

    for i in 0..3 {
        let req = RequestV2 {
            id: format!("v2-loop-{i}"),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text { text: "hi".into() }],
            }],
            ..Default::default()
        };
        let mut buf = Vec::with_capacity(256);
        write_frame(&mut buf, &req).expect("encode");
        stream.write_all(&buf).await.expect("write");
        stream.flush().await.expect("flush");
    }

    let (read_half, _w) = stream.into_split();
    let mut reader = BufReader::with_capacity(8 * 1024, read_half);
    let mut done_ids = Vec::new();
    let mut line = Vec::new();
    // Each request emits 1 Frame{Text} + 1 Done = 2 frames; 3
    // requests = 6 frames total.
    for _ in 0..6 {
        line.clear();
        let n = tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut line))
            .await
            .expect("read budget")
            .expect("read");
        assert!(n > 0, "EOF before all frames received");
        let resp: ResponseV2 = serde_json::from_slice(&line).expect("decode");
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

#[tokio::test]
async fn pre_stream_unavailable_returns_backend_unavailable() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["never".into()],
        pre_stream_error: Some(inferd_engine::mock::MockError::Unavailable),
        ..Default::default()
    }));
    let (addr, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let req = RequestV2 {
        id: "v2-fail".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        ..Default::default()
    };

    let resp = send_and_read_one(&addr, &req).await;
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

#[tokio::test]
async fn mid_stream_drop_emits_backend_unavailable_terminal() {
    // Mock yields 2 tokens then drops without Done. The daemon must
    // synthesise a terminal Error{BackendUnavailable,...} so the
    // client doesn't hang.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["a".into(), "b".into(), "c".into()],
        mid_stream_drop_after: Some(2),
        ..Default::default()
    }));
    let (addr, shutdown, handle) = boot_v2_daemon_with_mock(mock).await;

    let req = RequestV2 {
        id: "v2-mid".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        ..Default::default()
    };

    let frames = send_and_read_all(&addr, &req).await;
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
