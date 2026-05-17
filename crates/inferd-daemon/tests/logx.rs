//! M3 integration test: end-to-end activity log shape + redaction.
//!
//! Boots the lifecycle in-process against the mock backend over loopback
//! TCP (matching `tests/echo.rs`), but with the real `LogxLayer`
//! installed and pointed at a tempdir. Drives one full request through
//! the daemon, then reads the on-disk NDJSON and asserts:
//!
//! 1. A `request_done` record exists with the right fields and shape
//!    per `docs/protocol-v1.md` and `THREAT_MODEL.md` F-3.
//! 2. A synthetic credential injected via the request's prompt does not
//!    appear in the log file at any verbosity (proves F-3's write-time
//!    redactor runs end-to-end).

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::{serve_tcp, wait_for_ready, AcceptContext};
use inferd_daemon::logx::{LogxLayer, LogxWriter};
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::{write_frame, Message, Request, Response, Role};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing_subscriber::layer::SubscriberExt;

async fn boot_daemon(
    log_dir: &Path,
    mock_config: MockConfig,
) -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
    tracing::subscriber::DefaultGuard,
) {
    let writer = Arc::new(LogxWriter::open(log_dir, "inferd", 16 * 1024 * 1024).unwrap());
    let layer = LogxLayer::new(writer);
    // Scoped subscriber so two tests in the same process don't fight
    // over a global default. The activity-log layer wraps a registry
    // dedicated to this test.
    let subscriber = tracing_subscriber::registry().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);

    let mock = Arc::new(Mock::with_config(mock_config));
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

    (addr, shutdown_tx, handle, guard)
}

async fn send_request(addr: &str, req: &Request) -> Vec<Response> {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut buf = Vec::new();
    write_frame(&mut buf, req).expect("write frame");
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

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

fn read_log(dir: &Path) -> Vec<Value> {
    let path = dir.join("inferd.ndjson");
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("ndjson parse"))
        .collect()
}

#[tokio::test]
async fn request_done_record_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, handle, _guard) = boot_daemon(
        dir.path(),
        MockConfig {
            tokens: vec!["alpha".into(), "beta".into()],
            ..Default::default()
        },
    )
    .await;

    let frames = send_request(
        &addr,
        &Request {
            id: "log-r1".into(),
            messages: vec![Message {
                role: Role::User,
                content: "hi".into(),
            }],
            ..default_request()
        },
    )
    .await;
    assert_eq!(frames.len(), 3, "2 tokens + 1 done");

    // The accept loop spawns a per-connection task; allow tokio a moment
    // to flush the activity record after the response stream completed.
    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;

    let records = read_log(dir.path());
    let request_done = records
        .iter()
        .find(|r| r.get("msg").and_then(|m| m.as_str()) == Some("request_done"))
        .expect("expected a request_done record in the log");

    assert_eq!(request_done["req_id"], "log-r1");
    assert_eq!(request_done["backend"], "mock");
    assert_eq!(request_done["completion_tokens"], 2);
    assert!(request_done.get("t").and_then(|v| v.as_str()).is_some());
    assert_eq!(request_done["level"], "info");
    assert!(request_done.get("component").is_some());
}

#[tokio::test]
async fn injected_credential_does_not_leak_into_log() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, shutdown, handle, _guard) = boot_daemon(dir.path(), MockConfig::default()).await;

    // Synthetic credential; not a real key. Assembled at runtime so
    // secret-scanning tools don't flag the source file.
    let secret = format!("{}-{}", "sk", "1234567890abcdefghij");

    // Place the secret in a request id — that field flows through the
    // activity log directly. We're not embedding it in `content` because
    // mock doesn't echo content into log fields, but `req_id` is logged
    // verbatim on every request_done.
    let req_id = format!("req-with-secret-{secret}");

    let _ = send_request(
        &addr,
        &Request {
            id: req_id.clone(),
            messages: vec![Message {
                role: Role::User,
                content: "hi".into(),
            }],
            ..default_request()
        },
    )
    .await;

    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;

    // Read raw bytes (not parsed JSON) so we can scan for the literal.
    let raw = std::fs::read_to_string(dir.path().join("inferd.ndjson")).unwrap();
    assert!(
        !raw.contains(&secret),
        "secret leaked into activity log: {raw}"
    );
    assert!(
        raw.contains("[REDACTED"),
        "expected redaction marker in: {raw}"
    );
}

fn default_request() -> Request {
    Request {
        id: String::new(),
        messages: vec![],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    }
}
