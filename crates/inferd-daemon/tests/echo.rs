//! M1 exit-criterion integration test.
//!
//! Spins up the daemon's lifecycle (against the `mock` backend) over
//! loopback TCP, connects a real client, sends a `Request`, and asserts
//! the response stream is shaped per `docs/protocol-v1.md` and ADR 0008.
//!
//! Loopback TCP is used (rather than UDS) so this test runs unchanged on
//! Windows. UDS-specific paths are exercised by `endpoint::tests`.
//!
//! Coverage:
//! - End-to-end request → token(s) → done.
//! - Done frame carries `stop_reason: end` and `backend: mock` (ADR 0008).
//! - Frame `id` is echoed verbatim.
//! - `Request::resolve()` defaults are applied (we omit sampling fields
//!   and observe a successful generation).
//! - Queue-full path: with `MockConfig::tokens` set to a slow-ish stream
//!   and `active_permits=1`, a second submit while the first is in
//!   flight returns `code: queue_full`. (Skipped here — exercised at
//!   the queue unit level.)
//! - F-13 ready gating: connecting before the listener exists fails
//!   with the platform's ECONNREFUSED. Implicit — the test connects
//!   only after `bind_tcp` returns.

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::{serve_tcp, wait_for_ready};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::{write_frame, ErrorCode, Message, Request, Response, Role, StopReason};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

async fn boot_daemon(
    mock_config: MockConfig,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let mock = Arc::new(Mock::with_config(mock_config));
    let router = Arc::new(Router::new(vec![mock]));

    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp(listener, router, shutdown_rx).await;
    });

    (addr, shutdown_tx, handle)
}

async fn send_and_collect(addr: &str, req: &Request) -> Vec<Response> {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    // Write request frame.
    let mut buf = Vec::new();
    write_frame(&mut buf, req).expect("write frame");
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    // Read response frames until we hit a terminal one or EOF.
    let mut reader = BufReader::new(stream);
    let mut frames = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).await.expect("read");
        if n == 0 {
            break;
        }
        let resp: Response = serde_json::from_slice(&line).expect("decode");
        let terminal = resp.is_terminal();
        frames.push(resp);
        if terminal {
            break;
        }
    }
    frames
}

#[tokio::test]
async fn end_to_end_streams_tokens_then_done() {
    let (addr, shutdown, handle) = boot_daemon(MockConfig {
        tokens: vec!["alpha ".into(), "beta ".into(), "gamma".into()],
        ..Default::default()
    })
    .await;

    let req = Request {
        id: "req-1".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hello".into(),
        }],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    };

    let frames = send_and_collect(&addr, &req).await;
    assert_eq!(frames.len(), 4, "3 tokens + 1 done; got {frames:#?}");

    for f in &frames {
        assert_eq!(f.id(), "req-1");
    }

    // First three are tokens.
    for (i, expected) in ["alpha ", "beta ", "gamma"].iter().enumerate() {
        match &frames[i] {
            Response::Token { content, .. } => assert_eq!(content, expected),
            other => panic!("frame[{i}] expected Token, got {other:?}"),
        }
    }

    // Final is a done with backend + stop_reason populated per ADR 0008.
    match &frames[3] {
        Response::Done {
            content,
            stop_reason,
            backend,
            usage,
            ..
        } => {
            assert_eq!(content, "alpha beta gamma");
            assert_eq!(*stop_reason, StopReason::End);
            assert_eq!(backend, "mock");
            assert_eq!(usage.completion_tokens, 3);
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn invalid_request_yields_error_frame() {
    let (addr, shutdown, handle) = boot_daemon(MockConfig::default()).await;

    let req = Request {
        id: "bad".into(),
        messages: vec![],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    };

    let frames = send_and_collect(&addr, &req).await;
    assert_eq!(frames.len(), 1);
    match &frames[0] {
        Response::Error { id, code, message } => {
            assert_eq!(id, "bad");
            assert_eq!(*code, ErrorCode::InvalidRequest);
            assert!(message.contains("messages"), "message: {message}");
        }
        other => panic!("expected Error frame, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn mid_stream_drop_yields_backend_unavailable_error() {
    // Mock that emits 1 token then drops the stream without Done. Daemon
    // should surface a backend_unavailable error per ADR 0007.
    let (addr, shutdown, handle) = boot_daemon(MockConfig {
        tokens: vec!["partial".into(), "rest".into()],
        mid_stream_drop_after: Some(1),
        ..Default::default()
    })
    .await;

    let req = Request {
        id: "drop-1".into(),
        messages: vec![Message {
            role: Role::User,
            content: "x".into(),
        }],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    };

    let frames = send_and_collect(&addr, &req).await;
    // 1 token + 1 error.
    assert_eq!(frames.len(), 2, "got: {frames:#?}");
    match &frames[0] {
        Response::Token { content, .. } => assert_eq!(content, "partial"),
        other => panic!("frame[0] expected Token, got {other:?}"),
    }
    match &frames[1] {
        Response::Error { code, .. } => {
            assert_eq!(*code, ErrorCode::BackendUnavailable);
        }
        other => panic!("frame[1] expected Error, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

// THREAT_MODEL F-13: connecting before bind_tcp returns must fail. We can't
// easily test this in-process (we'd need to spy on the gating logic), but
// we can assert the inverse: bind_tcp does not return until wait_for_ready
// has returned. The test below proves wait_for_ready blocks on a non-ready
// backend, which is the gating condition the lifecycle relies on.
#[tokio::test]
async fn ready_gating_blocks_listener_creation_until_ready() {
    let mock = Arc::new(Mock::new());
    mock.set_ready(false);
    let router = Router::new(vec![Arc::clone(&mock) as _]);

    // wait_for_ready must NOT return while the backend is not ready.
    let res = tokio::time::timeout(
        Duration::from_millis(200),
        wait_for_ready(&router, Duration::from_secs(5)),
    )
    .await;
    assert!(res.is_err(), "wait_for_ready returned before ready");

    // Flip ready and confirm wait_for_ready completes.
    mock.set_ready(true);
    let elapsed = wait_for_ready(&router, Duration::from_secs(2))
        .await
        .unwrap();
    assert!(elapsed < Duration::from_millis(150));
}
