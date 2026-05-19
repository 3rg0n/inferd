//! THREAT_MODEL F-8 integration test: TCP API-key first-frame auth.
//!
//! Boots the lifecycle in-process with `AcceptContext::expected_api_key`
//! set, then confirms three behaviours over loopback TCP:
//!
//! 1. Client that sends the right auth frame proceeds normally.
//! 2. Client that sends a wrong key gets disconnected silently —
//!    no protocol error frame, no confirmation the endpoint exists.
//! 3. Client that skips auth and sends a normal Request gets the
//!    same silent close.

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::{AcceptContext, serve_tcp, wait_for_ready};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::{Message, Request, Response, Role, write_frame};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

async fn boot(
    api_key: Option<&str>,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let mock = Arc::new(Mock::with_config(MockConfig {
        tokens: vec!["ok".into()],
        ..Default::default()
    }));
    let router = Arc::new(Router::new(vec![mock]));
    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();

    let ctx = AcceptContext {
        expected_api_key: api_key.map(|s| s.to_string()),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp(listener, router, ctx, shutdown_rx).await;
    });
    (addr, shutdown_tx, handle)
}

fn req(id: &str) -> Request {
    Request {
        id: id.into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    }
}

async fn read_all_frames(stream: TcpStream) -> Vec<Response> {
    let mut reader = BufReader::new(stream);
    let mut frames = Vec::new();
    loop {
        let mut line = Vec::new();
        let n =
            match tokio::time::timeout(Duration::from_secs(2), reader.read_until(b'\n', &mut line))
                .await
            {
                Ok(Ok(n)) => n,
                _ => return frames,
            };
        if n == 0 {
            return frames;
        }
        match serde_json::from_slice::<Response>(&line) {
            Ok(resp) => {
                let terminal = resp.is_terminal();
                frames.push(resp);
                if terminal {
                    return frames;
                }
            }
            Err(_) => return frames,
        }
    }
}

#[tokio::test]
async fn correct_api_key_proceeds_to_request_handling() {
    let (addr, shutdown, handle) = boot(Some("super-secret")).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    // Auth frame.
    stream
        .write_all(b"{\"type\":\"auth\",\"key\":\"super-secret\"}\n")
        .await
        .unwrap();
    // Request frame.
    let mut buf = Vec::new();
    write_frame(&mut buf, &req("auth-ok-1")).expect("write");
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    let frames = read_all_frames(stream).await;
    assert!(
        frames.iter().any(|f| matches!(f, Response::Done { .. })),
        "expected a Done frame, got {frames:#?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn wrong_api_key_closes_silently() {
    let (addr, shutdown, handle) = boot(Some("super-secret")).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    stream
        .write_all(b"{\"type\":\"auth\",\"key\":\"WRONG\"}\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    write_frame(&mut buf, &req("auth-bad-1")).expect("write");
    let _ = stream.write_all(&buf).await; // may fail mid-write
    let _ = stream.flush().await;

    let frames = read_all_frames(stream).await;
    assert!(
        frames.is_empty(),
        "wrong key must produce no protocol frames; got {frames:#?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn skipping_auth_closes_silently() {
    let (addr, shutdown, handle) = boot(Some("super-secret")).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    // Send a normal request frame as the very first thing — auth-frame
    // parser sees `type=request`, returns None, daemon closes.
    let mut buf = Vec::new();
    write_frame(&mut buf, &req("no-auth-1")).expect("write");
    let _ = stream.write_all(&buf).await;
    let _ = stream.flush().await;

    let frames = read_all_frames(stream).await;
    assert!(
        frames.is_empty(),
        "missing auth must produce no protocol frames; got {frames:#?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn no_api_key_configured_means_no_auth_required() {
    // Sanity: when AcceptContext::expected_api_key is None, the daemon
    // skips the auth check entirely. This is the existing default and
    // is also exercised by tests/echo.rs; included here so the suite
    // for THIS file shows the gate is conditional.
    let (addr, shutdown, handle) = boot(None).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    let mut buf = Vec::new();
    write_frame(&mut buf, &req("no-key-1")).expect("write");
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    let frames = read_all_frames(stream).await;
    assert!(
        frames.iter().any(|f| matches!(f, Response::Done { .. })),
        "expected a Done frame, got {frames:#?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}
