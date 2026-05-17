//! M4 exit-criterion: end-to-end NDJSON over a Windows named pipe.
//!
//! Mirrors `tests/echo.rs` (which uses loopback TCP) but exercises
//! `lifecycle::serve_named_pipe` against a real Windows named-pipe
//! transport. Compiled in only on Windows; on other platforms the
//! test file is empty.
//!
//! Coverage:
//! - Multi-instance accept: connect, send, drop; connect again, send,
//!   read — confirms the bind-next-instance-immediately pattern in
//!   serve_named_pipe.
//! - Done frame carries `stop_reason: end` and `backend: mock` — same
//!   ADR 0008 compliance check as the TCP version.

#![cfg(windows)]

use inferd_daemon::endpoint::bind_named_pipe;
use inferd_daemon::lifecycle::{serve_named_pipe, wait_for_ready};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::{write_frame, Message, Request, Response, Role, StopReason};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
        let _ = serve_named_pipe(&path_for_task, first, router, shutdown_rx).await;
    });

    (pipe_path, shutdown_tx, handle)
}

async fn send_and_collect(path: &str, req: &Request) -> Vec<Response> {
    // Retry the connect a few times — Windows named-pipe open() can
    // briefly fail with "all instances are busy" if the server is
    // between accept and bind-next-instance.
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

    let mut buf = Vec::new();
    write_frame(&mut buf, req).expect("write frame");
    client.write_all(&buf).await.unwrap();
    client.flush().await.unwrap();

    let mut reader = BufReader::new(client);
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

    let req = Request {
        id: "req-pipe-1".into(),
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

    let frames = send_and_collect(&path, &req).await;
    assert_eq!(frames.len(), 4, "3 tokens + 1 done; got {frames:#?}");

    for f in &frames {
        assert_eq!(f.id(), "req-pipe-1");
    }

    match &frames[3] {
        Response::Done {
            content,
            stop_reason,
            backend,
            ..
        } => {
            assert_eq!(content, "alpha beta gamma");
            assert_eq!(*stop_reason, StopReason::End);
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

    let req = |id: &str| Request {
        id: id.into(),
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

    // First client.
    let frames1 = send_and_collect(&path, &req("a")).await;
    assert!(matches!(frames1.last(), Some(Response::Done { .. })));

    // Second client immediately after — proves serve_named_pipe rebound
    // the next instance after the first accept.
    let frames2 = send_and_collect(&path, &req("b")).await;
    assert!(matches!(frames2.last(), Some(Response::Done { .. })));
    assert_eq!(frames2.last().unwrap().id(), "b");

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}
