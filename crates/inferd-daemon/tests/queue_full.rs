//! Integration test: protocol-promised `queue_full` frame (v0.4 / ADR 0021).
//!
//! The daemon's wire spec promises a `ResponseV2::Error{code: queue_full}`
//! frame on admission overflow. This test pins the contract so the next
//! regression is caught at PR time.
//!
//! Setup:
//! - Mock backend with a long per-token delay so each in-flight request
//!   occupies its admission slot for measurable time.
//! - Admission gate sized at `active_permits=1, queue_depth=1` — total
//!   capacity 2 outstanding requests across the daemon.
//! - Three concurrent requests fired. Two get admitted; the third must
//!   come back with `code: queue_full`.

mod common;

#[cfg(unix)]
use common::{collect_frames, text_request};
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
#[cfg(unix)]
use inferd_daemon::lifecycle::wait_for_ready;
#[cfg(unix)]
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_uds_v2};
#[cfg(unix)]
use inferd_daemon::queue::Admission;
#[cfg(unix)]
use inferd_daemon::router::Router;
#[cfg(unix)]
use inferd_engine::mock::{Mock, MockConfig};
#[cfg(unix)]
use inferd_proto::v2::{ErrorCodeV2, ResponseV2};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::net::UnixStream;

/// 100ms per token + 8 tokens = ~800ms per request. Plenty of overlap
/// for three in-flight requests to race.
#[cfg(unix)]
const TOKEN_DELAY_MS: u64 = 100;
#[cfg(unix)]
const TOKENS_PER_REQUEST: usize = 8;

/// Spin up a daemon configured with admission capacity 2 (active=1
/// + queued=1). All requests share this gate.
#[cfg(unix)]
async fn boot_admission_capped_daemon() -> (
    std::path::PathBuf,
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

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let idx = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let socket_path = std::env::temp_dir().join(format!(
        "inferd-test-qfull-{}-{}.sock",
        std::process::id(),
        idx
    ));
    let _ = std::fs::remove_file(&socket_path);

    let listener = bind_uds(&socket_path, None).await.expect("bind uds");

    let admission = Admission::new(1, 1);
    let ctx = AcceptContext {
        admission: Some(admission),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_uds_v2(listener, router, ctx, shutdown_rx).await;
    });

    (socket_path, shutdown_tx, handle)
}

/// Send one request and collect every response frame until terminal.
#[cfg(unix)]
async fn one_request(path: std::path::PathBuf, id: String) -> Vec<ResponseV2> {
    let mut stream = UnixStream::connect(&path).await.expect("connect");
    common::write_request(&mut stream, &text_request(&id, "hi")).await;
    let (read_half, _w) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);
    collect_frames(&mut reader).await
}

#[cfg(unix)]
#[tokio::test]
async fn third_concurrent_request_gets_queue_full_when_capacity_is_two() {
    let (path, shutdown, handle) = boot_admission_capped_daemon().await;

    // Fire three concurrent requests. With capacity=2 the third should be
    // rejected at admission with code: queue_full.
    let tasks: Vec<_> = (0..3)
        .map(|i| {
            let path = path.clone();
            tokio::spawn(async move { one_request(path, format!("admission-{i}")).await })
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
            ResponseV2::Done { .. } => done_count += 1,
            ResponseV2::Error {
                code: ErrorCodeV2::QueueFull,
                ..
            } => queue_full_count += 1,
            other => panic!("client {i}: unexpected terminal {other:?}"),
        }
    }

    // At least one queue_full; every client terminates with done or
    // queue_full. (Not asserting exactly one — if scheduling lined up so
    // the first finished before the third's admission attempt, all three
    // could complete; unlikely at 800ms/request but not worth a flaky
    // hard assert.)
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

#[cfg(unix)]
#[tokio::test]
async fn queue_full_frame_includes_request_id() {
    // Capacity 1: every concurrent request beyond the first gets
    // queue_full. Fire two overlapping requests and assert the queue_full
    // frame echoes the right id.
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: (0..TOKENS_PER_REQUEST).map(|i| format!("t{i}")).collect(),
        token_delay_ms: Some(TOKEN_DELAY_MS),
        ..Default::default()
    }));
    let router = Arc::new(Router::new(vec![mock]));
    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    static COUNTER2: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let idx = COUNTER2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let socket_path = std::env::temp_dir().join(format!(
        "inferd-test-qfull-id-{}-{}.sock",
        std::process::id(),
        idx
    ));
    let _ = std::fs::remove_file(&socket_path);

    let listener = bind_uds(&socket_path, None).await.expect("bind uds");
    let ctx = AcceptContext {
        admission: Some(Admission::new(1, 0)),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_uds_v2(listener, router, ctx, shutdown_rx).await;
    });

    // Start the slow first request and let it claim the only slot.
    let path_first = socket_path.clone();
    let first = tokio::spawn(async move { one_request(path_first, "first".into()).await });
    // Brief delay so `first` is admitted before we send `second`.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let frames = tokio::time::timeout(
        Duration::from_secs(10),
        one_request(socket_path.clone(), "second".into()),
    )
    .await
    .expect("second request hung");

    let last = frames.last().expect("zero frames for second request");
    match last {
        ResponseV2::Error { id, code, .. } => {
            assert_eq!(id, "second", "queue_full frame must echo request id");
            assert_eq!(*code, ErrorCodeV2::QueueFull);
        }
        other => panic!("expected queue_full error frame, got {other:?}"),
    }

    let _ = first.await;
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
