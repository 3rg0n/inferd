//! Concurrency stress harness for the daemon's lifecycle.
//!
//! Targets the failure modes that v0.1.0 stable can't ship without
//! evidence they don't blow up:
//!
//! 1. **Many simultaneous connects.** N clients connect, each sends
//!    one request, each waits for a `done` frame. Asserts the
//!    accept loop doesn't lose connections, every request gets
//!    answered, no client sees an EOF mid-stream.
//! 2. **Mid-stream cancellation.** A client connects, sends a
//!    request, drops the connection while the backend is still
//!    streaming tokens. Asserts the daemon cleans up the in-flight
//!    job and remains responsive to subsequent connects.
//! 3. **Graceful shutdown with jobs in flight.** Shutdown signal
//!    fires while N requests are streaming. Asserts the daemon
//!    exits cleanly within a reasonable budget (no permanent hang).
//! 4. **Accept-loop pressure.** N clients hammer the listener with
//!    connect-disconnect churn (no request, just connect + close).
//!    Asserts the daemon survives without OOM / fd exhaustion.
//!
//! Uses the mock backend with `token_delay_ms` set so per-request
//! work is observable — without this, the entire stream completes
//! before the next connection lands and "concurrency" reduces to
//! "fast serial".
//!
//! Loopback TCP transport (port 0 to pick a free one). Runs
//! cross-platform without UDS / named-pipe special cases. The
//! NDJSON wire path is identical regardless of transport, so
//! lifecycle-level concurrency bugs surface here equally.
//!
//! These tests are deliberately probabilistic: the assertions
//! check *no failures occurred*, not exact timing. Tuning
//! constants below.

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::{AcceptContext, serve_tcp, wait_for_ready};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::{Message, Request, Response, Role, write_frame};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Number of concurrent client connections in the saturation test.
/// 50 is "well past plausible real-world demand on a dev machine"
/// without crossing into stress-tunable territory that would slow
/// down CI.
const N_CLIENTS: usize = 50;

/// Per-token delay for the mock backend during stress runs. Long
/// enough that requests overlap in the wire. Short enough that the
/// whole suite finishes well under one second of wall time per test.
const TOKEN_DELAY: Duration = Duration::from_millis(5);

/// Per-test wall-clock budget. Generous — these tests aren't about
/// exact latency, they're about "does it finish at all under load."
const TEST_BUDGET: Duration = Duration::from_secs(30);

/// Spin up a daemon with the mock backend that takes
/// `TOKEN_DELAY` per token, return its bound address + a handle to
/// shut it down. Caller is expected to send the shutdown signal
/// before the test returns so the listener task exits cleanly.
async fn boot_stress_daemon(
    tokens_per_request: usize,
) -> (
    String,
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

    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp(listener, router, AcceptContext::default(), shutdown_rx).await;
    });

    (addr, shutdown_tx, handle)
}

/// Send one request, return the parsed response frames in order.
/// Retries the connect briefly on transient failures so CI runners
/// under load don't false-fail when the listener is busy.
async fn one_request(addr: String, id: String) -> Vec<Response> {
    let mut stream = None;
    for attempt in 0..10 {
        match TcpStream::connect(&addr).await {
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

    let req = Request {
        id: id.clone(),
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
async fn fifty_concurrent_clients_each_get_a_done_frame() {
    let (addr, shutdown, handle) = boot_stress_daemon(8).await;
    let start = Instant::now();

    let tasks: Vec<_> = (0..N_CLIENTS)
        .map(|i| {
            let addr = addr.clone();
            tokio::spawn(async move { one_request(addr, format!("stress-{i}")).await })
        })
        .collect();

    // Bound the test to keep CI sane.
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

    // Every client must have received at least one Token plus a Done.
    let mut done_count = 0usize;
    let mut error_count = 0usize;
    for (i, frames) in all_results.iter().enumerate() {
        let has_done = frames.iter().any(|f| matches!(f, Response::Done { .. }));
        let has_error = frames.iter().any(|f| matches!(f, Response::Error { .. }));
        let token_count = frames
            .iter()
            .filter(|f| matches!(f, Response::Token { .. }))
            .count();
        if has_done {
            done_count += 1;
            assert!(
                token_count > 0,
                "client {i}: got Done with no preceding Token frames"
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

#[tokio::test]
async fn mid_stream_disconnect_does_not_break_the_daemon() {
    // 64 tokens × 5ms = ~320ms per request, plenty of time to drop
    // mid-stream.
    let (addr, shutdown, handle) = boot_stress_daemon(64).await;

    // Issue a request, read the first token, then drop the connection
    // mid-stream. Repeat 20 times.
    for i in 0..20 {
        let mut stream = TcpStream::connect(&addr).await.expect("connect");
        let req = Request {
            id: format!("cancel-{i}"),
            messages: vec![Message {
                role: Role::User,
                content: "x".into(),
            }],
            ..Default::default()
        };
        let mut buf = Vec::with_capacity(256);
        write_frame(&mut buf, &req).unwrap();
        stream.write_all(&buf).await.unwrap();
        stream.flush().await.unwrap();

        let (read_half, _write_half) = stream.into_split();
        let mut reader = BufReader::with_capacity(8 * 1024, read_half);
        let mut line = Vec::new();
        // Pull the first token, then drop the entire stream.
        let _ = tokio::time::timeout(Duration::from_secs(1), reader.read_until(b'\n', &mut line))
            .await
            .expect("first token timeout");
        // `reader` and the underlying socket drop here.
    }

    // After 20 mid-stream cancellations the daemon must still be
    // serving. Give in-flight handler tasks a moment to drain
    // (slow CI runners overlap teardown with the next connect),
    // then issue one more request and read it through to Done.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let frames = tokio::time::timeout(TEST_BUDGET, one_request(addr.clone(), "post-cancel".into()))
        .await
        .expect("daemon hung after cancellations");

    let has_done = frames.iter().any(|f| matches!(f, Response::Done { .. }));
    assert!(
        has_done,
        "daemon failed to serve a request after 20 mid-stream cancellations: frames={frames:?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn shutdown_with_jobs_in_flight_completes_quickly() {
    // 32 tokens × 5ms = ~160ms per request.
    let (addr, shutdown, handle) = boot_stress_daemon(32).await;

    // Fire N concurrent requests but do NOT await them — we want
    // jobs in flight when the shutdown fires.
    let _bg_tasks: Vec<_> = (0..N_CLIENTS)
        .map(|i| {
            let addr = addr.clone();
            tokio::spawn(async move {
                let _ = one_request(addr, format!("inflight-{i}")).await;
            })
        })
        .collect();

    // Give the daemon a moment to actually accept everyone.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Trigger shutdown.
    let shutdown_started = Instant::now();
    let _ = shutdown.send(());

    // Wait for the listener task to finish. It should drop in-flight
    // connections and exit quickly.
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

#[tokio::test]
async fn connect_churn_does_not_leak_resources() {
    // Two-token requests so each is fast; the goal is high churn.
    let (addr, shutdown, handle) = boot_stress_daemon(2).await;

    // 200 connect-and-immediately-close cycles. No request payload
    // sent — purely accept-loop pressure.
    for _ in 0..200 {
        let stream = TcpStream::connect(&addr).await.expect("connect");
        drop(stream);
    }

    // Daemon should still be alive and serving. Same drain delay
    // as the cancellation test — accept-loop is independent of
    // handler tasks but the OS may still be cleaning up the 200
    // closed sockets when our follow-up connect lands.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let frames = tokio::time::timeout(TEST_BUDGET, one_request(addr.clone(), "post-churn".into()))
        .await
        .expect("daemon hung after connect churn");

    let has_done = frames.iter().any(|f| matches!(f, Response::Done { .. }));
    assert!(
        has_done,
        "daemon failed to serve a request after connect churn: frames={frames:?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
