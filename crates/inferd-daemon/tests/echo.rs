//! End-to-end exit-criterion integration test (v0.4 / ADR 0021).
//!
//! Spins up the daemon's v2 lifecycle (against the `mock` backend) over
//! Unix domain socket, connects over the length-prefixed wire, sends a
//! `RequestV2`, and asserts the response stream is shaped per ADR 0015
//! (typed content blocks) framed per ADR 0021 (length-prefixed frames).
//!
//! Unix domain sockets are used per ADR 0022 (inbound TCP removed).
//! Platform-specific tests: `echo_pipe.rs` for Windows.
//!
//! Coverage:
//! - End-to-end request → text frame(s) → done.
//! - Done frame carries `stop_reason: end_turn` and `backend: mock`.
//! - Frame `id` is echoed verbatim.
//! - Empty-messages request → `Error{InvalidRequest}`.
//! - Mid-stream backend drop → terminal `Error{BackendUnavailable}`.
//! - F-13 ready gating: `wait_for_ready` blocks on a non-ready backend.

mod common;

#[cfg(unix)]
use common::{collect_frames, text_request, write_request};
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
#[cfg(unix)]
use inferd_daemon::lifecycle::wait_for_ready;
#[cfg(unix)]
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_uds_v2};
#[cfg(unix)]
use inferd_daemon::router::Router;
#[cfg(unix)]
use inferd_engine::mock::{Mock, MockConfig};
#[cfg(unix)]
use inferd_proto::v2::{ErrorCodeV2, RequestV2, ResponseBlock, ResponseV2, StopReasonV2};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(unix)]
async fn boot_daemon(
    mock_config: MockConfig,
) -> (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let mock = Arc::new(Mock::with_config(mock_config));
    let router = Arc::new(Router::new(vec![mock]));

    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    let socket_path = std::env::temp_dir().join(format!("inferd-test-{}.sock", std::process::id()));
    // Clean up any stale socket file.
    let _ = std::fs::remove_file(&socket_path);

    let listener = bind_uds(&socket_path, None).await.expect("bind uds");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_uds_v2(listener, router, AcceptContext::default(), shutdown_rx).await;
    });

    (socket_path, shutdown_tx, handle)
}

#[cfg(unix)]
async fn send_and_collect(path: &std::path::Path, req: &RequestV2) -> Vec<ResponseV2> {
    let mut stream = UnixStream::connect(path).await.expect("connect");
    write_request(&mut stream, req).await;
    let (read_half, _write_half) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);
    collect_frames(&mut reader).await
}

#[cfg(unix)]
#[tokio::test]
async fn end_to_end_streams_text_then_done() {
    let (addr, shutdown, handle) = boot_daemon(MockConfig {
        tokens: vec!["alpha ".into(), "beta ".into(), "gamma".into()],
        ..Default::default()
    })
    .await;

    let frames = send_and_collect(&addr, &text_request("req-1", "hello")).await;
    assert_eq!(frames.len(), 4, "3 text frames + 1 done; got {frames:#?}");

    for f in &frames {
        assert_eq!(f.id(), "req-1");
    }

    // First three are incremental text frames.
    for (i, expected) in ["alpha ", "beta ", "gamma"].iter().enumerate() {
        match &frames[i] {
            ResponseV2::Frame {
                block: ResponseBlock::Text { delta },
                ..
            } => assert_eq!(delta, expected),
            other => panic!("frame[{i}] expected Frame{{Text}}, got {other:?}"),
        }
    }

    // Final is a done with backend + stop_reason populated.
    match &frames[3] {
        ResponseV2::Done {
            stop_reason,
            backend,
            usage,
            ..
        } => {
            assert_eq!(*stop_reason, StopReasonV2::EndTurn);
            assert_eq!(backend, "mock");
            assert_eq!(usage.output_tokens, 3);
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_request_yields_error_frame() {
    let (addr, shutdown, handle) = boot_daemon(MockConfig::default()).await;

    let req = RequestV2 {
        id: "bad".into(),
        messages: vec![],
        ..Default::default()
    };

    let frames = send_and_collect(&addr, &req).await;
    assert_eq!(frames.len(), 1);
    match &frames[0] {
        ResponseV2::Error { id, code, message } => {
            assert_eq!(id, "bad");
            assert_eq!(*code, ErrorCodeV2::InvalidRequest);
            assert!(message.contains("messages"), "message: {message}");
        }
        other => panic!("expected Error frame, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn mid_stream_drop_yields_backend_unavailable_error() {
    // Mock that emits 1 text frame then drops the stream without Done.
    // Daemon should surface a backend_unavailable error per ADR 0007.
    let (addr, shutdown, handle) = boot_daemon(MockConfig {
        tokens: vec!["partial".into(), "rest".into()],
        mid_stream_drop_after: Some(1),
        ..Default::default()
    })
    .await;

    let frames = send_and_collect(&addr, &text_request("drop-1", "x")).await;
    // 1 text frame + 1 error.
    assert_eq!(frames.len(), 2, "got: {frames:#?}");
    match &frames[0] {
        ResponseV2::Frame {
            block: ResponseBlock::Text { delta },
            ..
        } => assert_eq!(delta, "partial"),
        other => panic!("frame[0] expected Frame{{Text}}, got {other:?}"),
    }
    match &frames[1] {
        ResponseV2::Error { code, .. } => {
            assert_eq!(*code, ErrorCodeV2::BackendUnavailable);
        }
        other => panic!("frame[1] expected Error, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

// THREAT_MODEL F-13: connecting before bind_uds returns must fail. We
// can't easily test this in-process, but we can assert the gating
// condition: wait_for_ready blocks on a non-ready backend and completes
// once it flips ready.
#[cfg(unix)]
#[tokio::test]
async fn ready_gating_blocks_listener_creation_until_ready() {
    let mock = Arc::new(Mock::new());
    mock.set_ready(false);
    let router = Router::new(vec![Arc::clone(&mock) as _]);

    let res = tokio::time::timeout(
        Duration::from_millis(200),
        wait_for_ready(&router, Duration::from_secs(5)),
    )
    .await;
    assert!(res.is_err(), "wait_for_ready returned before ready");

    mock.set_ready(true);
    let elapsed = wait_for_ready(&router, Duration::from_secs(2))
        .await
        .unwrap();
    assert!(elapsed < Duration::from_millis(150));
}
