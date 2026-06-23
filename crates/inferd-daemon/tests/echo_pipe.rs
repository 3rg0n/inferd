//! Exit-criterion: end-to-end length-prefixed v2 over a Windows named
//! pipe (v0.4 / ADR 0021).
//!
//! Mirrors `tests/echo.rs` (loopback TCP) but exercises
//! `lifecycle_v2::serve_named_pipe_v2` against a real Windows named-pipe
//! transport. Compiled in only on Windows; on other platforms the file
//! is empty.
//!
//! Coverage:
//! - Multi-instance accept: connect, send, read; connect again, send,
//!   read — confirms the bind-next-instance-immediately pattern in
//!   serve_named_pipe_v2.
//! - Done frame carries `stop_reason: end_turn` and `backend: mock`.

#![cfg(windows)]

mod common;

use common::{collect_frames, text_request, write_request};
use inferd_daemon::endpoint::bind_named_pipe;
use inferd_daemon::lifecycle::wait_for_ready;
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_named_pipe_v2};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::v2::{RequestV2, ResponseV2, StopReasonV2};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::windows::named_pipe::ClientOptions;

fn unique_pipe_path() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Process-wide atomic counter guarantees uniqueness across parallel
    // tests in the same binary; ts + pid spread the namespace across
    // independent processes.
    format!(r"\\.\pipe\inferd-test-{pid}-{ts}-{n}")
}

async fn boot_daemon(
    pipe_path: String,
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

    // Pre-bind the first server instance so the listener exists before
    // boot_daemon returns; eliminates the race where a client connects
    // between tokio::spawn() and the first bind_named_pipe call.
    let first = bind_named_pipe(&pipe_path, true).expect("bind first pipe instance");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let path_for_task = pipe_path.clone();
    let handle = tokio::spawn(async move {
        let _ = serve_named_pipe_v2(
            &path_for_task,
            first,
            router,
            AcceptContext::default(),
            shutdown_rx,
        )
        .await;
    });

    (pipe_path, shutdown_tx, handle)
}

async fn send_and_collect(path: &str, req: &RequestV2) -> Vec<ResponseV2> {
    // Retry the connect a few times — Windows named-pipe open() can
    // briefly fail with "all instances are busy" if the server is between
    // accept and bind-next-instance.
    let mut client = None;
    for attempt in 0..20 {
        match ClientOptions::new().open(path) {
            Ok(c) => {
                client = Some(c);
                break;
            }
            Err(e) if attempt < 19 => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = e;
            }
            Err(e) => panic!("client open failed: {e}"),
        }
    }
    let mut client = client.expect("client connected");

    write_request(&mut client, req).await;
    let (read_half, _w) = tokio::io::split(client);
    let mut reader = tokio::io::BufReader::new(read_half);
    collect_frames(&mut reader).await
}

#[tokio::test]
async fn end_to_end_over_named_pipe() {
    let path = unique_pipe_path();
    let (path, shutdown, handle) = boot_daemon(
        path,
        MockConfig {
            tokens: vec!["alpha ".into(), "beta ".into(), "gamma".into()],
            ..Default::default()
        },
    )
    .await;

    let frames = send_and_collect(&path, &text_request("req-pipe-1", "hello")).await;
    assert_eq!(frames.len(), 4, "3 text frames + 1 done; got {frames:#?}");

    for f in &frames {
        assert_eq!(f.id(), "req-pipe-1");
    }

    match &frames[3] {
        ResponseV2::Done {
            stop_reason,
            backend,
            ..
        } => {
            assert_eq!(*stop_reason, StopReasonV2::EndTurn);
            assert_eq!(backend, "mock");
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn multi_instance_accept_serves_two_sequential_clients() {
    let path = unique_pipe_path();
    let (path, shutdown, handle) = boot_daemon(
        path,
        MockConfig {
            tokens: vec!["one".into()],
            ..Default::default()
        },
    )
    .await;

    // First client.
    let frames1 = send_and_collect(&path, &text_request("a", "x")).await;
    assert!(matches!(frames1.last(), Some(ResponseV2::Done { .. })));

    // Second client immediately after — proves serve_named_pipe_v2 rebound
    // the next instance after the first accept.
    let frames2 = send_and_collect(&path, &text_request("b", "x")).await;
    assert!(matches!(frames2.last(), Some(ResponseV2::Done { .. })));
    assert_eq!(frames2.last().unwrap().id(), "b");

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}
