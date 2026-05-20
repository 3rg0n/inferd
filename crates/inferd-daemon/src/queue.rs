//! Bounded admission gate.
//!
//! Per `docs/protocol-v1.md` §"Admission semantics": at most
//! `active_permits + queue_depth` outstanding requests across the
//! whole daemon at any time. The (active_permits + 1)th request is
//! still admitted (it just queues behind the active one); the
//! (active_permits + queue_depth + 1)th is rejected immediately
//! with `Response::Error{code: queue_full}`.
//!
//! Implementation: a single `tokio::sync::Semaphore` whose total
//! permit count is `active_permits + queue_depth`. Per-request
//! flow:
//! 1. `try_acquire_owned` — non-blocking. If no permit available,
//!    return `QueueFull`. The wire layer translates that into a
//!    terminal `Response::Error` frame.
//! 2. Hold the permit for the duration of the generation (the
//!    `OwnedSemaphorePermit` guard goes onto the request future's
//!    stack).
//! 3. Drop on completion / cancellation. Permit returns to the
//!    pool, freeing slot for the next admit.
//!
//! For v0.1 the daemon's only backend (`llamacpp`) is single-
//! threaded internally — concurrent generates serialise on its
//! inner mutex. Setting `active_permits=1` matches that reality
//! without bottlenecking the wire layer (which is happy to read
//! and queue many requests). v0.2's continuous-batching backends
//! will raise `active_permits` above 1.

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

/// Errors returned by `Admission::try_admit`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SubmitError {
    /// Admission gate is at capacity (active + queued). The wire
    /// layer must respond with `Response::Error{code: queue_full}`
    /// and close the request stream.
    #[error("queue full")]
    QueueFull,
    /// Admission gate has been shut down (semaphore closed).
    /// Submits during shutdown are rejected.
    #[error("queue closed")]
    Closed,
}

/// Shared admission gate handed to every per-connection task via
/// `lifecycle::AcceptContext`. Cheap to clone (just an `Arc` bump).
#[derive(Clone)]
pub struct Admission {
    inner: Arc<Semaphore>,
    /// Cached for `capacity()` reporting; the semaphore itself
    /// doesn't expose its initial size.
    capacity: usize,
}

impl Admission {
    /// Build an admission gate sized for `active_permits +
    /// queue_depth` simultaneous outstanding requests across the
    /// daemon.
    pub fn new(active_permits: usize, queue_depth: usize) -> Self {
        // Saturate at 1 below; the v0.1 invariant is active=1, but
        // operators can pass 0 by accident (env var unset → "0"
        // parse on some setups). Treat as "at least one slot."
        let total = active_permits.max(1) + queue_depth;
        Self {
            inner: Arc::new(Semaphore::new(total)),
            capacity: total,
        }
    }

    /// Attempt to admit one request. Non-blocking. The returned
    /// `OwnedSemaphorePermit` must be held for the duration of the
    /// generation; dropping it returns the slot to the pool.
    pub fn try_admit(&self) -> Result<OwnedSemaphorePermit, SubmitError> {
        match Arc::clone(&self.inner).try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(TryAcquireError::NoPermits) => Err(SubmitError::QueueFull),
            Err(TryAcquireError::Closed) => Err(SubmitError::Closed),
        }
    }

    /// Total slots configured (active_permits + queue_depth).
    /// Diagnostic only; the wire layer doesn't surface this.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Approximate count of slots currently free. Diagnostic;
    /// racy by definition (a concurrent admit can change the
    /// answer between the call and the use).
    pub fn available_permits(&self) -> usize {
        self.inner.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_succeeds_until_capacity() {
        let a = Admission::new(1, 2);
        assert_eq!(a.capacity(), 3);
        let _p1 = a.try_admit().unwrap();
        let _p2 = a.try_admit().unwrap();
        let _p3 = a.try_admit().unwrap();
        // Fourth admit fails — over capacity.
        assert_eq!(a.try_admit().unwrap_err(), SubmitError::QueueFull);
    }

    #[test]
    fn dropping_permit_frees_slot() {
        let a = Admission::new(1, 1);
        let p1 = a.try_admit().unwrap();
        let _p2 = a.try_admit().unwrap();
        assert!(a.try_admit().is_err());
        drop(p1);
        // Slot now available.
        let _p3 = a.try_admit().unwrap();
    }

    #[test]
    fn zero_active_permits_treated_as_one() {
        // Operator misconfigured to 0; we still admit at least one
        // slot (queue_depth=0 + 1 active = 1 total). Otherwise the
        // daemon would refuse every request which is worse than a
        // silent floor.
        let a = Admission::new(0, 0);
        assert_eq!(a.capacity(), 1);
        let _p = a.try_admit().unwrap();
        assert!(a.try_admit().is_err());
    }

    #[test]
    fn available_permits_reflects_holds() {
        let a = Admission::new(1, 2);
        assert_eq!(a.available_permits(), 3);
        let _p1 = a.try_admit().unwrap();
        assert_eq!(a.available_permits(), 2);
        let _p2 = a.try_admit().unwrap();
        assert_eq!(a.available_permits(), 1);
    }

    #[tokio::test]
    async fn admission_is_clone_safe_across_tasks() {
        let a = Admission::new(1, 2);
        let a2 = a.clone();
        let h = tokio::spawn(async move {
            let _p = a2.try_admit().unwrap();
            // Permit drops on task exit.
        });
        h.await.unwrap();
        // After the spawned task drops its permit, full capacity
        // is back.
        let p1 = a.try_admit().unwrap();
        let p2 = a.try_admit().unwrap();
        let p3 = a.try_admit().unwrap();
        drop((p1, p2, p3));
    }
}
