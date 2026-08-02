//! Daemon lifecycle primitives shared across every wire surface:
//! readiness gating ([`wait_for_ready`]) and the per-accept
//! [`AcceptContext`].
//!
//! As of v0.4 (ADR 0021) the generation connection handler and its
//! listeners live in [`crate::lifecycle_v2`] — the original text-only
//! v1 NDJSON path was removed when v1 was folded into v2. This module
//! keeps only the transport-agnostic pieces the v2 and embed
//! lifecycles both build on:
//! - `lock` — single-instance lock at startup (THREAT_MODEL F-2).
//! - `router` — backend selection.
//! - `endpoint` — listener bound only after `router.all_ready()`
//!   (THREAT_MODEL F-13).
//! - `queue` — admission gate (`SubmitError::QueueFull` → wire
//!   `code: queue_full`).

use crate::queue::Admission;
use crate::router::Router;
use std::time::{Duration, Instant};

/// Wait until every backend in `router` reports ready, polling at 50ms
/// intervals up to `timeout`. Returns the duration spent waiting.
///
/// THREAT_MODEL F-13: nothing else creates listeners until this returns.
pub async fn wait_for_ready(router: &Router, timeout: Duration) -> Result<Duration, ReadyTimeout> {
    let started = Instant::now();
    let poll = Duration::from_millis(50);
    loop {
        if router.all_ready() {
            return Ok(started.elapsed());
        }
        if started.elapsed() >= timeout {
            return Err(ReadyTimeout(timeout));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Returned when `wait_for_ready` exhausts its budget without seeing
/// readiness across every backend.
#[derive(Debug, thiserror::Error)]
#[error("backend not ready within {0:?}")]
pub struct ReadyTimeout(pub Duration);

/// Default ceiling on a single response write (THREAT_MODEL F-17).
///
/// A response write blocks once the peer's receive buffer fills and the
/// peer stops reading. Because that write happens while the request
/// holds its admission permit, an unbounded wait converts one
/// stopped-reading client into a permanently occupied generation slot.
/// This bounds the wait.
///
/// 60s is chosen to be far longer than any legitimate write can take —
/// a response frame is at most a few hundred KiB and the peer is a local
/// process, so the only way to exceed it is a peer that has stopped
/// reading entirely — while still short enough that a wedged slot
/// recovers without operator intervention.
pub const DEFAULT_WRITE_TIMEOUT_SECS: u64 = 60;

/// Per-accept context that the lifecycle hands to every spawned
/// connection task.
///
/// Today it carries the shared admission gate (queue_full enforcement)
/// and the per-write timeout (F-17). New per-connection policy (rate
/// limits, per-caller quotas) extends this struct rather than each
/// `serve_*` signature. (Peer identity is kernel-attested per transport
/// — UDS/pipe, F-7 — so there is no in-band API-key field; inbound TCP
/// was removed in ADR 0022.)
#[derive(Clone)]
pub struct AcceptContext {
    /// Shared admission gate. `None` for tests / dev paths that
    /// don't care about queue depth — those treat every request
    /// as admitted. Production lifecycle always passes `Some`.
    pub admission: Option<Admission>,
    /// Ceiling on one response write, after which the connection is
    /// dropped and the admission permit released (THREAT_MODEL F-17).
    /// Defaults to [`DEFAULT_WRITE_TIMEOUT_SECS`]; tests shorten it to
    /// make the wedge reproducible in bounded time. `None` disables the
    /// bound — never set by the daemon binary, and only correct for a
    /// test that owns both ends of the socket.
    pub write_timeout: Option<Duration>,
}

impl Default for AcceptContext {
    fn default() -> Self {
        Self {
            admission: None,
            write_timeout: Some(Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS)),
        }
    }
}

impl std::fmt::Debug for AcceptContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptContext")
            .field(
                "admission_capacity",
                &self.admission.as_ref().map(|a| a.capacity()),
            )
            .field("write_timeout", &self.write_timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_engine::mock::Mock;
    use std::sync::Arc;

    #[tokio::test]
    async fn wait_for_ready_returns_when_already_ready() {
        let router = Router::new(vec![Arc::new(Mock::new())]);
        let elapsed = wait_for_ready(&router, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(elapsed < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_not_ready() {
        let mock = Arc::new(Mock::new());
        mock.set_ready(false);
        let router = Router::new(vec![mock]);
        let err = wait_for_ready(&router, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not ready"));
    }

    #[tokio::test]
    async fn wait_for_ready_succeeds_after_delayed_ready() {
        let mock = Arc::new(Mock::new());
        mock.set_ready(false);
        let router = Router::new(vec![mock.clone()]);

        let m2 = Arc::clone(&mock);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            m2.set_ready(true);
        });

        let elapsed = wait_for_ready(&router, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(elapsed >= Duration::from_millis(100));
    }
}
