//! Integration test: protocol-promised `queue_full` frame.
//!
//! Until the admission gate landed (see `lifecycle::handle_connection`
//! and `queue::Admission`), the daemon's wire spec promised a
//! `Response::Error{code: queue_full}` frame on overflow but never
//! emitted one. This test pins the contract so the next regression
//! is caught at PR time.
//!
//! Setup:
//! - Mock backend with a long per-token delay so each in-flight
//!   request occupies its admission slot for measurable time.
//! - Admission gate sized at `active_permits=1, queue_depth=1` —
//!   total capacity 2 outstanding requests across the daemon.
//! - Three concurrent requests fired. Two get admitted; the third
//!   must come back with `code: queue_full`.

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::{AcceptContext, serve_tcp, wait_for_ready};
use inferd_daemon::queue::Admission;
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::{ErrorCode, Message, Request, Response, Role, write_frame};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// 100ms per token + 8 tokens = ~800ms per request. Plenty of
/// overlap for three in-flight requests to race.
const TOKEN_DELAY_MS: u64 = 100;
const TOKENS_PER_REQUEST: usize = 8;

/// Spin up a daemon configured with admission capacity 2 (active=1
/// + queued=1). All requests share this gate.
async fn boot_admission_capped_daemon() -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: (0..TOKENS_PER_REQUEST).map(|i| format!("t{i}")).collect(),
        token_delay_ms: Some(TOKEN_DELAY_MS),
        ..Default::default()
    }));
    let router = Arc::new(Router::new(vec![mock]));

    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();

    let admission = Admission::new(1, 1);
    let ctx = AcceptContext {
        expected_api_key: None,
        admission: Some(admission),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp(listener, router, ctx, shutdown_rx).await;
    });

    (addr, shutdown_tx, handle)
}

/// Send one request and collect every response frame until terminal.
async fn one_request(addr: String, id: String) -> Vec<Response> {
    let mut stream = TcpStream::connect(&addr).await.expect("connect");

    let req = Request {
        id,
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        ..Default::default()
    };
    let mut buf = Vec::with_capacity(256);
    write_frame(&mut buf, &req).expect("encode request");
    stream.write_all(&buf).await.expect("write request");
    stream.flush().await.expect("flush");

    let (read_half, _write_half) = stream.into_split();
    let mut reader = BufReader::with_capacity(8 * 1024, read_half);
    let mut frames = Vec::new();
    let mut line = Vec::with_capacity(512);
    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .await
            .expect("read frame");
        if n == 0 {
            break;
        }
        let resp: Response = serde_json::from_slice(&line).expect("decode response frame");
        let terminal = matches!(&resp, Response::Done { .. } | Response::Error { .. });
        frames.push(resp);
        if terminal {
            break;
        }
    }
    frames
}

#[tokio::test]
async fn third_concurrent_request_gets_queue_full_when_capacity_is_two() {
    let (addr, shutdown, handle) = boot_admission_capped_daemon().await;

    // Fire three concurrent requests. With capacity=2 the third
    // should be rejected at admission with code: queue_full.
    let tasks: Vec<_> = (0..3)
        .map(|i| {
            let addr = addr.clone();
            tokio::spawn(async move { one_request(addr, format!("admission-{i}")).await })
        })
        .collect();

    let mut all_results = Vec::with_capacity(3);
    for t in tasks {
        let res = tokio::time::timeout(Duration::from_secs(30), t)
            .await
            .expect("test budget exceeded — daemon hung?")
            .expect("client task panic");
        all_results.push(res);
    }

    let mut done_count = 0;
    let mut queue_full_count = 0;
    for (i, frames) in all_results.iter().enumerate() {
        let last = frames
            .last()
            .unwrap_or_else(|| panic!("client {i}: zero frames"));
        match last {
            Response::Done { .. } => done_count += 1,
            Response::Error {
                code: ErrorCode::QueueFull,
                ..
            } => queue_full_count += 1,
            other => panic!("client {i}: unexpected terminal {other:?}"),
        }
    }

    // Exactly one queue_full, exactly two completed normally.
    // (Or all three could complete if scheduling lined up so the
    // first finished before the third's admission attempt — unlikely
    // with 800ms per request but worth not asserting it harshly.)
    assert!(
        queue_full_count >= 1,
        "expected at least one queue_full; got done={done_count} queue_full={queue_full_count}"
    );
    assert_eq!(
        done_count + queue_full_count,
        3,
        "every client must terminate with done or queue_full"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn queue_full_frame_includes_request_id() {
    // Capacity 1: every concurrent request beyond the first
    // gets queue_full. Fire two sequentially-overlapping requests
    // and assert the queue_full frame echoes the right id.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: (0..TOKENS_PER_REQUEST).map(|i| format!("t{i}")).collect(),
        token_delay_ms: Some(TOKEN_DELAY_MS),
        ..Default::default()
    }));
    let router = Arc::new(Router::new(vec![mock]));
    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();
    let ctx = AcceptContext {
        expected_api_key: None,
        admission: Some(Admission::new(1, 0)),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp(listener, router, ctx, shutdown_rx).await;
    });

    // Start the slow first request and let it claim the only slot.
    let addr_first = addr.clone();
    let first = tokio::spawn(async move { one_request(addr_first, "first".into()).await });
    // Brief delay so `first` is admitted before we send `second`.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let frames = tokio::time::timeout(
        Duration::from_secs(10),
        one_request(addr.clone(), "second".into()),
    )
    .await
    .expect("second request hung");

    let last = frames.last().expect("zero frames for second request");
    match last {
        Response::Error { id, code, .. } => {
            assert_eq!(id, "second", "queue_full frame must echo request id");
            assert_eq!(*code, ErrorCode::QueueFull);
        }
        other => panic!("expected queue_full error frame, got {other:?}"),
    }

    let _ = first.await;
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
