//! Regression test: a peer that stops reading must not hold an
//! admission permit indefinitely (THREAT_MODEL F-17).
//!
//! The wedge this pins down: response writes happen *after* the admission
//! gate, while `_admit_permit` is alive. A client that sends a valid
//! request, gets admitted, and then stops reading its socket fills the
//! kernel's send buffer; the daemon's `write_all` blocks; and the permit
//! is never released. With `active_permits + queue_depth` such clients,
//! generation is denied to every other consumer on the machine and no
//! timeout exists to break it.
//!
//! Setup:
//! - Admission capacity 1 (`active=1, queued=0`) so a single wedged
//!   permit is the whole gate.
//! - A short `write_timeout` on `AcceptContext` so the bound is
//!   observable in test time rather than the 60s default.
//! - A mock backend emitting enough token frames to overflow the peer's
//!   receive buffer, so the daemon genuinely blocks in `write_all`
//!   rather than completing the response into the socket buffer.
//!
//! The assertion is about the *gate*, not the wedged client: a
//! well-behaved client, retrying past `queue_full` within a bounded
//! budget, must eventually be served. With the write bound in place it is
//! served shortly after the timeout fires and the permit drops. Without
//! it, every attempt inside the budget comes back `queue_full` — the
//! permit is never released — and the test fails.
//!
//! Runs on both transports: UDS on Unix, named pipe on Windows.

mod common;

use common::{collect_frames, text_request};
use inferd_daemon::lifecycle::wait_for_ready;
use inferd_daemon::lifecycle_v2::AcceptContext;
use inferd_daemon::queue::Admission;
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig};
use inferd_proto::v2::ResponseV2;
use std::sync::Arc;
use std::time::Duration;

/// How long the daemon may block on one response write before giving up.
/// Short enough to keep the test quick, long enough that a healthy
/// request on a local socket never trips it.
const WRITE_TIMEOUT: Duration = Duration::from_millis(600);

/// Enough token frames, each large enough, to exceed any platform's
/// default socket/pipe buffer — so the daemon is genuinely blocked in
/// `write_all` against a non-reading peer rather than having buffered the
/// whole response.
const STALL_TOKENS: usize = 512;
const STALL_TOKEN_BYTES: usize = 4096;

fn stalling_mock() -> Arc<Mock> {
    Arc::new(Mock::with_config(MockConfig {
        tokens: (0..STALL_TOKENS)
            .map(|_| "x".repeat(STALL_TOKEN_BYTES))
            .collect(),
        ..Default::default()
    }))
}

/// A capacity-1 admission gate plus the short write bound under test.
fn wedge_ctx() -> AcceptContext {
    AcceptContext {
        admission: Some(Admission::new(1, 0)),
        write_timeout: Some(WRITE_TIMEOUT),
    }
}

/// Budget for the victim's retry loop. Generous relative to
/// `WRITE_TIMEOUT` so a passing run isn't timing-sensitive, but finite so
/// a wedged gate fails rather than hangs.
const VICTIM_BUDGET: Duration = Duration::from_secs(20);
const VICTIM_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Assert that `attempt` — one full request/response round trip — is
/// eventually served with a `Done`, retrying while it returns
/// `queue_full`. A wedged admission permit makes every attempt return
/// `queue_full` until the budget runs out.
async fn victim_is_eventually_served<F, Fut>(mut attempt: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<ResponseV2>>,
{
    let deadline = tokio::time::Instant::now() + VICTIM_BUDGET;
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        let frames = attempt().await;
        match frames.last() {
            Some(ResponseV2::Done { .. }) => return,
            Some(ResponseV2::Error {
                code: inferd_proto::v2::ErrorCodeV2::QueueFull,
                ..
            }) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "victim starved after {attempts} attempts over {VICTIM_BUDGET:?}: the \
                     non-reading peer never released its admission permit"
                );
                tokio::time::sleep(VICTIM_RETRY_INTERVAL).await;
            }
            other => panic!("victim got an unexpected terminal frame: {other:?}"),
        }
    }
}

#[cfg(unix)]
mod uds {
    use super::*;
    use inferd_daemon::endpoint::bind_uds;
    use inferd_daemon::lifecycle_v2::serve_uds_v2;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    fn socket_path() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let idx = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "inferd-test-wstall-{}-{}.sock",
            std::process::id(),
            idx
        ))
    }

    #[tokio::test]
    async fn non_reading_peer_does_not_wedge_the_admission_gate() {
        let router = Arc::new(Router::new(vec![stalling_mock()]));
        wait_for_ready(&router, Duration::from_secs(2))
            .await
            .expect("backend ready");

        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        let listener = bind_uds(&path, None).await.expect("bind uds");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let serve = tokio::spawn({
            let router = Arc::clone(&router);
            async move {
                let _ = serve_uds_v2(listener, router, wedge_ctx(), shutdown_rx).await;
            }
        });

        // The hostile client: send a valid request, take the only
        // admission slot, then never read a byte.
        let mut hostile = UnixStream::connect(&path).await.expect("hostile connect");
        common::write_request(&mut hostile, &text_request("hostile", "hi")).await;
        hostile.flush().await.expect("flush hostile request");

        // Give the daemon time to admit the request and block in write_all
        // against the unread socket.
        tokio::time::sleep(WRITE_TIMEOUT / 2).await;

        // A well-behaved client must get served once the bound fires and
        // the permit is released.
        victim_is_eventually_served(|| async {
            let mut stream = UnixStream::connect(&path).await.expect("victim connect");
            common::write_request(&mut stream, &text_request("victim", "hi")).await;
            let (read_half, _w) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(read_half);
            collect_frames(&mut reader).await
        })
        .await;

        drop(hostile);
        let _ = shutdown_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), serve).await;
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(windows)]
mod pipe {
    use super::*;
    use inferd_daemon::endpoint::bind_named_pipe;
    use inferd_daemon::lifecycle_v2::serve_named_pipe_v2;
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ClientOptions;

    fn pipe_path() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!(
            r"\\.\pipe\inferd-test-wstall-{}-{ts}-{n}",
            std::process::id()
        )
    }

    /// Open a client instance, retrying "all pipe instances are busy"
    /// while the server is between accept and bind-next-instance.
    async fn connect(path: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
        for attempt in 0..50 {
            match ClientOptions::new().open(path) {
                Ok(c) => return c,
                Err(e) if attempt < 49 => {
                    let _ = e;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => panic!("client open failed: {e}"),
            }
        }
        unreachable!("retry loop always returns or panics")
    }

    #[tokio::test]
    async fn non_reading_peer_does_not_wedge_the_admission_gate() {
        let router = Arc::new(Router::new(vec![stalling_mock()]));
        wait_for_ready(&router, Duration::from_secs(2))
            .await
            .expect("backend ready");

        let path = pipe_path();
        let first = bind_named_pipe(&path, true).expect("bind first pipe instance");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let serve = tokio::spawn({
            let router = Arc::clone(&router);
            let path = path.clone();
            async move {
                let _ = serve_named_pipe_v2(&path, first, router, wedge_ctx(), shutdown_rx).await;
            }
        });

        let mut hostile = connect(&path).await;
        common::write_request(&mut hostile, &text_request("hostile", "hi")).await;
        hostile.flush().await.expect("flush hostile request");

        tokio::time::sleep(WRITE_TIMEOUT / 2).await;

        victim_is_eventually_served(|| async {
            let mut client = connect(&path).await;
            common::write_request(&mut client, &text_request("victim", "hi")).await;
            let (read_half, _w) = tokio::io::split(client);
            let mut reader = tokio::io::BufReader::new(read_half);
            collect_frames(&mut reader).await
        })
        .await;

        drop(hostile);
        let _ = shutdown_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), serve).await;
    }
}
