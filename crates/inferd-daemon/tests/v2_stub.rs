//! Integration test for the Phase 1B v2 socket binding.
//!
//! Pins the contract that the v2 listener:
//!   - accepts NDJSON connections on its own socket (separate from v1);
//!   - parses `RequestV2` frames;
//!   - returns `ResponseV2::Error{InvalidRequest, ...}` for structurally
//!     invalid requests;
//!   - returns `ResponseV2::Error{Internal, "v2 generation not
//!     implemented..."}` for valid requests until Phase 2A.
//!
//! Once Phase 2A lands and the daemon dispatches v2 to the backend,
//! the `valid_request_*` tests below need to be updated to expect
//! actual response frames.

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_tcp_v2};
use inferd_proto::v2::{
    Attachment, AttachmentKind, ContentBlock, ErrorCodeV2, MessageV2, RequestV2, ResponseV2, RoleV2,
};
use inferd_proto::write_frame;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

async fn boot_v2_daemon() -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();
    let ctx = AcceptContext {
        expected_api_key: None,
        admission: None,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp_v2(listener, ctx, shutdown_rx).await;
    });
    (addr, shutdown_tx, handle)
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

#[tokio::test]
async fn valid_v2_request_returns_internal_not_implemented() {
    let (addr, shutdown, handle) = boot_v2_daemon().await;

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

    let resp = send_and_read_one(&addr, &req).await;
    match resp {
        ResponseV2::Error { id, code, message } => {
            assert_eq!(id, "v2-001");
            assert_eq!(code, ErrorCodeV2::Internal);
            assert!(
                message.contains("not implemented"),
                "message should explain Phase 2A is pending; got: {message}"
            );
        }
        other => panic!("expected Error{{Internal,..}}, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn valid_multimodal_v2_request_validates_then_returns_internal() {
    // Confirms attachment routing past resolve() — the request is
    // structurally valid (image content block matches a resident
    // attachment), so we expect Internal not InvalidRequest.
    let (addr, shutdown, handle) = boot_v2_daemon().await;

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
        attachments: vec![Attachment {
            id: "img-1".into(),
            kind: AttachmentKind::Image,
            mime: "image/jpeg".into(),
            bytes: "base64data".into(),
        }],
        ..Default::default()
    };

    let resp = send_and_read_one(&addr, &req).await;
    match resp {
        ResponseV2::Error { id, code, .. } => {
            assert_eq!(id, "v2-002");
            assert_eq!(code, ErrorCodeV2::Internal);
        }
        other => panic!("expected Error{{Internal,..}}, got {other:?}"),
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
async fn multiple_requests_on_one_v2_connection_each_get_their_own_frame() {
    // After a valid-but-unimplemented request, the daemon should keep
    // the connection open and process the next frame. (Only frame-
    // decode errors close the connection.)
    let (addr, shutdown, handle) = boot_v2_daemon().await;

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
    for i in 0..3 {
        let mut line = Vec::new();
        let n = tokio::time::timeout(Duration::from_secs(5), reader.read_until(b'\n', &mut line))
            .await
            .expect("read budget")
            .expect("read");
        assert!(n > 0, "expected frame {i}, got EOF");
        let resp: ResponseV2 = serde_json::from_slice(&line).expect("decode");
        match resp {
            ResponseV2::Error { id, code, .. } => {
                assert_eq!(id, format!("v2-loop-{i}"));
                assert_eq!(code, ErrorCodeV2::Internal);
            }
            other => panic!("frame {i}: expected Error{{Internal,..}}, got {other:?}"),
        }
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
