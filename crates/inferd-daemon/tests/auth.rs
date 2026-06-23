//! THREAT_MODEL F-8 integration test: TCP API-key first-frame auth
//! (v0.4 / ADR 0021 length-prefixed wire).
//!
//! Boots the v2 lifecycle in-process with `AcceptContext::expected_api_key`
//! set, then confirms three behaviours over loopback TCP:
//!
//! 1. Client that sends the right auth frame proceeds normally.
//! 2. Client that sends a wrong key gets disconnected silently —
//!    no protocol error frame, no confirmation the endpoint exists.
//! 3. Client that skips auth and sends a normal request gets the
//!    same silent close.

mod common;

use common::{collect_frames, text_request, write_auth, write_request};
use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::wait_for_ready;
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_tcp_v2};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::v2::ResponseV2;
use std::sync::Arc;
use std::time::Duration;
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
        admission: None,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp_v2(listener, router, ctx, shutdown_rx).await;
    });
    (addr, shutdown_tx, handle)
}

/// Collect frames with a short timeout; a silent close yields an empty
/// vec rather than hanging.
async fn read_all_frames(stream: TcpStream) -> Vec<ResponseV2> {
    let (read_half, _w) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);
    tokio::time::timeout(Duration::from_secs(2), collect_frames(&mut reader))
        .await
        .unwrap_or_default()
}

#[tokio::test]
async fn correct_api_key_proceeds_to_request_handling() {
    let (addr, shutdown, handle) = boot(Some("super-secret")).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    write_auth(&mut stream, "super-secret").await;
    write_request(&mut stream, &text_request("auth-ok-1", "hi")).await;

    let frames = read_all_frames(stream).await;
    assert!(
        frames.iter().any(|f| matches!(f, ResponseV2::Done { .. })),
        "expected a Done frame, got {frames:#?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test]
async fn wrong_api_key_closes_silently() {
    let (addr, shutdown, handle) = boot(Some("super-secret")).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    write_auth(&mut stream, "WRONG").await;
    // Request may fail mid-write once the daemon drops the connection.
    write_request(&mut stream, &text_request("auth-bad-1", "hi")).await;

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
    // Send a normal request frame as the very first thing — the auth-frame
    // parser sees a non-auth payload, returns None, daemon closes.
    write_request(&mut stream, &text_request("no-auth-1", "hi")).await;

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
    // skips the auth check entirely. This is the existing default and is
    // also exercised by tests/echo.rs; included here so the suite for
    // THIS file shows the gate is conditional.
    let (addr, shutdown, handle) = boot(None).await;
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    write_request(&mut stream, &text_request("no-key-1", "hi")).await;

    let frames = read_all_frames(stream).await;
    assert!(
        frames.iter().any(|f| matches!(f, ResponseV2::Done { .. })),
        "expected a Done frame, got {frames:#?}"
    );

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}
