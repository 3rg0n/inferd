//! Integration test: end-to-end activity log shape + redaction
//! (v0.4 / ADR 0021).
//!
//! Boots the v2 lifecycle in-process against the mock backend over
//! Unix domain socket (matching `tests/echo.rs`), but with the real
//! `LogxLayer` installed and pointed at a tempdir. Drives one full
//! request through the daemon, then reads the on-disk NDJSON and asserts:
//!
//! 1. A `v2_request_done` record exists with the right fields and shape
//!    per `THREAT_MODEL.md` F-3.
//! 2. A synthetic credential injected via the request id does not appear
//!    in the log file (proves F-3's write-time redactor runs end-to-end).

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
use inferd_daemon::logx::{LogxLayer, LogxWriter};
#[cfg(unix)]
use inferd_daemon::router::Router;
#[cfg(unix)]
use inferd_engine::mock::{Mock, MockConfig};
#[cfg(unix)]
use inferd_proto::v2::{RequestV2, ResponseV2};
#[cfg(unix)]
use serde_json::Value;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tracing_subscriber::layer::SubscriberExt;

#[cfg(unix)]
async fn boot_daemon(
    log_dir: &Path,
    mock_config: MockConfig,
) -> (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
    tracing::subscriber::DefaultGuard,
) {
    let writer = Arc::new(LogxWriter::open(log_dir, "inferd", 16 * 1024 * 1024).unwrap());
    let layer = LogxLayer::new(writer);
    // Scoped subscriber so two tests in the same process don't fight over
    // a global default. The activity-log layer wraps a registry dedicated
    // to this test.
    let subscriber = tracing_subscriber::registry().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);

    let mock = Arc::new(Mock::with_config(mock_config));
    let router = Arc::new(Router::new(vec![mock]));
    wait_for_ready(&router, Duration::from_secs(2))
        .await
        .expect("backend ready");

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let idx = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let socket_path = std::env::temp_dir().join(format!(
        "inferd-test-logx-{}-{}.sock",
        std::process::id(),
        idx
    ));
    let _ = std::fs::remove_file(&socket_path);

    let listener = bind_uds(&socket_path, None).await.expect("bind uds");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_uds_v2(listener, router, AcceptContext::default(), shutdown_rx).await;
    });

    (socket_path, shutdown_tx, handle, guard)
}

#[cfg(unix)]
async fn send_request(path: &std::path::Path, req: &RequestV2) -> Vec<ResponseV2> {
    let mut stream = UnixStream::connect(path).await.expect("connect");
    write_request(&mut stream, req).await;
    let (read_half, _w) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);
    collect_frames(&mut reader).await
}

#[cfg(unix)]
fn read_log(dir: &Path) -> Vec<Value> {
    let path = dir.join("inferd.ndjson");
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("ndjson parse"))
        .collect()
}

#[cfg(unix)]
#[tokio::test]
async fn request_done_record_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (path, shutdown, handle, _guard) = boot_daemon(
        dir.path(),
        MockConfig {
            tokens: vec!["alpha".into(), "beta".into()],
            ..Default::default()
        },
    )
    .await;

    let frames = send_request(&path, &text_request("log-r1", "hi")).await;
    assert_eq!(frames.len(), 3, "2 text frames + 1 done");

    // The accept loop spawns a per-connection task; allow tokio a moment
    // to flush the activity record after the response stream completed.
    let _ = shutdown.send(());
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;

    let records = read_log(dir.path());
    let request_done = records
        .iter()
        .find(|r| r.get("msg").and_then(|m| m.as_str()) == Some("v2_request_done"))
        .expect("expected a v2_request_done record in the log");

    assert_eq!(request_done["req_id"], "log-r1");
    assert_eq!(request_done["backend"], "mock");
    assert_eq!(request_done["output_tokens"], 2);
    assert!(request_done.get("t").and_then(|v| v.as_str()).is_some());
    assert_eq!(request_done["level"], "info");
    assert!(request_done.get("component").is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn injected_credential_does_not_leak_into_log() {
    let dir = tempfile::tempdir().unwrap();
    let (path, shutdown, handle, _guard) = boot_daemon(dir.path(), MockConfig::default()).await;

    // Synthetic credential; not a real key. Assembled at runtime so
    // secret-scanning tools don't flag the source file.
    let secret = format!("{}-{}", "sk", "1234567890abcdefghij");

    // Place the secret in a request id — that field flows through the
    // activity log directly (`req_id` is logged verbatim on every
    // v2_request_done).
    let req_id = format!("req-with-secret-{secret}");
    let _ = send_request(&path, &text_request(&req_id, "hi")).await;

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
