//! Concurrency stress harness for the daemon's lifecycle (v0.4 / ADR 0021).
//!
//! Targets the failure modes a stable release can't ship without
//! evidence they don't blow up:
//!
//! 1. **Many simultaneous connects.** N clients connect, each sends one
//!    request, each waits for a `done` frame. Asserts the accept loop
//!    doesn't lose connections, every request gets answered, no client
//!    sees an EOF mid-stream.
//! 2. **Mid-stream cancellation.** A client connects, sends a request,
//!    drops the connection while the backend is still streaming. Asserts
//!    the daemon cleans up the in-flight job and stays responsive.
//! 3. **Graceful shutdown with jobs in flight.** Shutdown fires while N
//!    requests are streaming; the daemon exits cleanly within budget.
//! 4. **Accept-loop pressure.** N connect-disconnect churns (no request).
//!    Asserts the daemon survives without fd exhaustion.
//!
//! Uses the mock backend with `token_delay_ms` set so per-request work
//! is observable. Unix domain socket transport per ADR 0022; the
//! length-prefixed wire is identical regardless of transport, so
//! lifecycle-level concurrency bugs surface here equally.

mod common;

#[cfg(unix)]
use common::{collect_frames, read_lp_frame, text_request, write_request};
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
use inferd_proto::v2::ResponseV2;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::{Duration, Instant};
#[cfg(unix)]
use tokio::net::UnixStream;

/// Number of concurrent client connections in the saturation test.
#[cfg(unix)]
const N_CLIENTS: usize = 50;

/// Per-token delay for the mock backend during stress runs. Long enough
/// that requests overlap on the wire, short enough that the suite stays
/// well under one second of wall time per test.
#[cfg(unix)]
const TOKEN_DELAY: Duration = Duration::from_millis(5);

/// Per-test wall-clock budget. Generous — these tests are about "does it
/// finish at all under load," not exact latency.
#[cfg(unix)]
const TEST_BUDGET: Duration = Duration::from_secs(30);

#[cfg(unix)]
async fn boot_stress_daemon(
    tokens_per_request: usize,
) -> (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: (0..tokens_per_request).map(|i| format!("t{i}")).collect(),
        token_delay_ms: Some(TOKEN_DELAY.as_millis() as u64),
        ..Default::default()
    }));
    let router = Arc::new(Router::new(vec![mock]));

    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let idx = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let socket_path = std::env::temp_dir().join(format!(
        "inferd-test-stress-{}-{}.sock",
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

/// Send one request, return the parsed response frames in order. Retries
/// the connect briefly on transient failures so loaded CI runners don't
/// false-fail when the listener is busy.
#[cfg(unix)]
async fn one_request(path: std::path::PathBuf, id: String) -> Vec<ResponseV2> {
    let mut stream = None;
    for attempt in 0..10 {
        match UnixStream::connect(&path).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) if attempt < 9 => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("connect after retries: {e}"),
        }
    }
    let mut stream = stream.expect("connect");

    write_request(&mut stream, &text_request(&id, "hi")).await;
    let (read_half, _w) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);
    collect_frames(&mut reader).await
}

#[cfg(unix)]
#[tokio::test]
async fn fifty_concurrent_clients_each_get_a_done_frame() {
    let (path, shutdown, handle) = boot_stress_daemon(8).await;
    let start = Instant::now();

    let tasks: Vec<_> = (0..N_CLIENTS)
        .map(|i| {
            let path = path.clone();
            tokio::spawn(async move { one_request(path, format!("stress-{i}")).await })
        })
        .collect();

    let mut all_results = Vec::with_capacity(N_CLIENTS);
    for t in tasks {
        let res = tokio::time::timeout(TEST_BUDGET, t)
            .await
            .expect("test budget exceeded — daemon hung?")
            .expect("client task panic");
        all_results.push(res);
    }

    eprintln!(
        "fifty_concurrent_clients: wall time = {:?}",
        start.elapsed()
    );

    // Every client must have received at least one Text frame plus a Done.
    let mut done_count = 0usize;
    let mut error_count = 0usize;
    for (i, frames) in all_results.iter().enumerate() {
        let has_done = frames.iter().any(|f| matches!(f, ResponseV2::Done { .. }));
        let has_error = frames.iter().any(|f| matches!(f, ResponseV2::Error { .. }));
        let text_count = frames
            .iter()
            .filter(|f| matches!(f, ResponseV2::Frame { .. }))
            .count();
        if has_done {
            done_count += 1;
            assert!(
                text_count > 0,
                "client {i}: got Done with no preceding Frame frames"
            );
        }
        if has_error {
            error_count += 1;
        }
    }
    assert_eq!(
        done_count + error_count,
        N_CLIENTS,
        "every client must terminate with Done or Error; got {done_count} done + {error_count} error / {N_CLIENTS}"
    );
    assert!(
        error_count == 0,
        "some clients got error frames: {error_count}/{N_CLIENTS}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn mid_stream_disconnect_does_not_break_the_daemon() {
    // 64 tokens × 5ms = ~320ms per request, plenty of time to drop
    // mid-stream.
    let (path, shutdown, handle) = boot_stress_daemon(64).await;

    // Issue a request, read the first frame, then drop the connection
    // mid-stream. Repeat 20 times.
    for i in 0..20 {
        let mut stream = UnixStream::connect(&path).await.expect("connect");
        write_request(&mut stream, &text_request(&format!("cancel-{i}"), "x")).await;

        let (read_half, _w) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(read_half);
        // Pull the first frame, then drop the entire stream.
        let _ = tokio::time::timeout(Duration::from_secs(1), read_lp_frame(&mut reader))
            .await
            .expect("first frame timeout");
        // `reader` and the underlying socket drop here.
    }

    // After 20 mid-stream cancellations the daemon must still be serving.
    // Give in-flight handler tasks a moment to drain, then issue one more
    // request and read it through to Done.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let frames = tokio::time::timeout(TEST_BUDGET, one_request(path.clone(), "post-cancel".into()))
        .await
        .expect("daemon hung after cancellations");

    let has_done = frames.iter().any(|f| matches!(f, ResponseV2::Done { .. }));
    assert!(
        has_done,
        "daemon failed to serve a request after 20 mid-stream cancellations: frames={frames:?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_with_jobs_in_flight_completes_quickly() {
    // 32 tokens × 5ms = ~160ms per request.
    let (path, shutdown, handle) = boot_stress_daemon(32).await;

    // Fire N concurrent requests but do NOT await them — we want jobs in
    // flight when the shutdown fires.
    let _bg_tasks: Vec<_> = (0..N_CLIENTS)
        .map(|i| {
            let path = path.clone();
            tokio::spawn(async move {
                let _ = one_request(path, format!("inflight-{i}")).await;
            })
        })
        .collect();

    // Give the daemon a moment to actually accept everyone.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let shutdown_started = Instant::now();
    let _ = shutdown.send(());

    let res = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        res.is_ok(),
        "listener task did not exit within 5s of shutdown; daemon hung?"
    );
    eprintln!(
        "shutdown_with_jobs_in_flight: shutdown took {:?}",
        shutdown_started.elapsed()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn connect_churn_does_not_leak_resources() {
    // Two-token requests so each is fast; the goal is high churn.
    let (path, shutdown, handle) = boot_stress_daemon(2).await;

    // 200 connect-and-immediately-close cycles. No request payload sent —
    // purely accept-loop pressure.
    for _ in 0..200 {
        let stream = UnixStream::connect(&path).await.expect("connect");
        drop(stream);
    }

    // Daemon should still be alive and serving.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let frames = tokio::time::timeout(TEST_BUDGET, one_request(path.clone(), "post-churn".into()))
        .await
        .expect("daemon hung after connect churn");

    let has_done = frames.iter().any(|f| matches!(f, ResponseV2::Done { .. }));
    assert!(
        has_done,
        "daemon failed to serve a request after connect churn: frames={frames:?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
