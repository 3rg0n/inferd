//! Connect-and-retry helpers per `docs/protocol-v1.md` §"Client
//! connection lifecycle".

use crate::client::{Client, ClientError};
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::Instant;

/// Errors produced by `dial_and_wait_ready`.
#[derive(Debug, thiserror::Error)]
pub enum WaitError {
    /// Deadline elapsed before any successful connect.
    #[error("timed out after {0:?} waiting for inferd to become ready")]
    Timeout(Duration),
    /// A non-transient error surfaced — daemon is broken or
    /// permissions are wrong; not worth retrying.
    #[error("permanent connect error: {0}")]
    Permanent(ClientError),
}

/// Pattern A passive readiness: retry connect against the inference
/// transport until success or `timeout` elapses. Successful connect
/// is the ready signal — the daemon's inference socket only exists
/// when the backend is `ready` per THREAT_MODEL F-13.
///
/// Backoff schedule: 100ms initial, doubling each attempt, capped
/// at 5s. Permanent errors (permission denied, malformed addr,
/// decode failure) bubble up immediately as
/// `WaitError::Permanent`.
///
/// `dial_fn` is a closure producing a fresh dial future on each
/// attempt. This indirection lets callers swap transports without
/// us duplicating the retry loop:
///
/// ```no_run
/// use inferd_client::{dial_and_wait_ready, Client};
/// use std::time::Duration;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let client = dial_and_wait_ready(
///     Duration::from_secs(30),
///     || Client::dial_tcp("127.0.0.1:47321"),
/// )
/// .await?;
/// # Ok(()) }
/// ```
pub async fn dial_and_wait_ready<F, Fut>(
    timeout: Duration,
    mut dial_fn: F,
) -> Result<Client, WaitError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Client, ClientError>>,
{
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_millis(100);
    let max_delay = Duration::from_secs(5);

    loop {
        match dial_fn().await {
            Ok(c) => return Ok(c),
            Err(e) if !is_transient_dial_error(&e) => {
                return Err(WaitError::Permanent(e));
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    return Err(WaitError::Timeout(timeout));
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }
}

/// Returns `true` if `err` is the kind of transient connect failure
/// that the daemon's F-13 ready-gating produces during bring-up
/// (the inference socket doesn't exist yet). Permanent errors
/// (permission denied, malformed addr) return `false` and bubble
/// up immediately rather than spamming retries.
pub fn is_transient_dial_error(err: &ClientError) -> bool {
    let ClientError::Io(io_err) = err else {
        return false;
    };
    use std::io::ErrorKind;
    matches!(
        io_err.kind(),
        ErrorKind::ConnectionRefused
            | ErrorKind::NotFound
            | ErrorKind::TimedOut
            | ErrorKind::AddrNotAvailable
    ) || {
        // Windows pipe-busy comes through as raw os error; check
        // the message as a fallback. Rare but real.
        let msg = io_err.to_string().to_ascii_lowercase();
        msg.contains("all pipe instances are busy")
            || msg.contains("the system cannot find")
            || msg.contains("target machine actively refused")
    }
}

/// Default admin endpoint path per platform. Mirrors the daemon's
/// `endpoint::default_admin_addr` so clients can reach the spec'd
/// default without hard-coding it.
pub fn default_admin_addr() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/run/inferd/admin.sock")
    }
    #[cfg(target_os = "macos")]
    {
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push("admin.sock");
        p
    }
    #[cfg(windows)]
    {
        PathBuf::from(r"\\.\pipe\inferd-admin")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        PathBuf::from("/tmp/inferd/admin.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn io_err(kind: io::ErrorKind, msg: &str) -> ClientError {
        ClientError::Io(io::Error::new(kind, msg))
    }

    #[test]
    fn refused_is_transient() {
        assert!(is_transient_dial_error(&io_err(
            io::ErrorKind::ConnectionRefused,
            "refused"
        )));
    }

    #[test]
    fn notfound_is_transient() {
        assert!(is_transient_dial_error(&io_err(
            io::ErrorKind::NotFound,
            "no such file"
        )));
    }

    #[test]
    fn permission_denied_is_permanent() {
        assert!(!is_transient_dial_error(&io_err(
            io::ErrorKind::PermissionDenied,
            "denied"
        )));
    }

    #[test]
    fn pipe_busy_message_recognised_as_transient() {
        assert!(is_transient_dial_error(&io_err(
            io::ErrorKind::Other,
            "All pipe instances are busy."
        )));
    }

    #[test]
    fn decode_error_is_permanent() {
        // Synthesize a serde error by parsing garbage.
        let err: serde_json::Error = serde_json::from_str::<u32>("not a number").unwrap_err();
        let cerr = ClientError::Decode(err);
        assert!(!is_transient_dial_error(&cerr));
    }

    #[tokio::test]
    async fn dial_and_wait_ready_succeeds_first_try() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let dial = move || {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            // Build a minimal Client wrapping an in-memory pipe pair.
            let (a, _b) = tokio::io::duplex(64);
            let (read, write) = tokio::io::split(a);
            async move { Ok(Client::wrap_for_test(Box::new(read), Box::new(write))) }
        };
        let _ = dial_and_wait_ready(Duration::from_secs(1), dial)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dial_and_wait_ready_retries_transient() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let dial = move || {
            let n = calls_clone.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(io_err(io::ErrorKind::ConnectionRefused, "refused"))
                } else {
                    let (a, _b) = tokio::io::duplex(64);
                    let (read, write) = tokio::io::split(a);
                    Ok(Client::wrap_for_test(Box::new(read), Box::new(write)))
                }
            }
        };
        let _ = dial_and_wait_ready(Duration::from_secs(5), dial)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn dial_and_wait_ready_returns_permanent_immediately() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let dial = move || {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            async move { Err::<Client, _>(io_err(io::ErrorKind::PermissionDenied, "denied")) }
        };
        let err = dial_and_wait_ready(Duration::from_secs(5), dial)
            .await
            .unwrap_err();
        match err {
            WaitError::Permanent(_) => {}
            other => panic!("expected Permanent, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dial_and_wait_ready_times_out() {
        let dial = move || async move {
            Err::<Client, _>(io_err(io::ErrorKind::ConnectionRefused, "refused"))
        };
        let err = dial_and_wait_ready(Duration::from_millis(250), dial)
            .await
            .unwrap_err();
        assert!(matches!(err, WaitError::Timeout(_)));
    }
}
