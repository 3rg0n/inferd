//! Bounded admission queue.
//!
//! Per `docs/protocol-v1.md` §"Admission semantics": 1 active generation,
//! N queued (default 10), non-blocking submit. Queue full returns
//! `SubmitError::QueueFull` immediately so the caller can emit
//! `Response::Error{code: queue_full}`.
//!
//! The queue is transport-agnostic. It does not know about NDJSON or
//! sockets — it accepts opaque jobs and hands them to a worker one at a
//! time. The lifecycle wires this up to the inference backend.

use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

/// Errors returned by `Queue::submit`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SubmitError {
    /// Queue is full at the configured depth. Caller should not retry
    /// without backoff.
    #[error("queue full")]
    QueueFull,
    /// Queue has been shut down. Submits after shutdown are rejected.
    #[error("queue closed")]
    Closed,
}

/// A bounded FIFO admission queue.
///
/// The queue bounds two things separately:
/// - **active generations**: at most `active_permits` simultaneously
///   (default 1, the v0.1 invariant).
/// - **queued jobs**: at most `queue_depth` waiting for a permit.
///
/// `submit()` is non-blocking: if no permit is immediately available **and**
/// the queue is at depth, it returns `QueueFull`. The caller never blocks.
pub struct Queue<T: Send + 'static> {
    tx: mpsc::Sender<T>,
    permits: Arc<Semaphore>,
}

impl<T: Send + 'static> Queue<T> {
    /// Build a queue with the given active-permit count and waiting depth.
    ///
    /// Returns the queue plus the receiving end of the channel; the worker
    /// loop in `lifecycle::dispatch_loop` consumes from `rx` and calls
    /// `permits.acquire()` before each job.
    pub fn new(active_permits: usize, queue_depth: usize) -> (Self, mpsc::Receiver<T>) {
        // mpsc capacity is the *waiting* queue depth, not active+queued.
        // Active jobs are tracked separately via the semaphore.
        let (tx, rx) = mpsc::channel(queue_depth.max(1));
        let permits = Arc::new(Semaphore::new(active_permits.max(1)));
        (Queue { tx, permits }, rx)
    }

    /// Submit a job non-blocking.
    pub fn submit(&self, job: T) -> Result<(), SubmitError> {
        match self.tx.try_send(job) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(SubmitError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SubmitError::Closed),
        }
    }

    /// Acquire a permit for the worker loop. Held for the duration of one
    /// active generation; dropped (returned to the pool) when the generation
    /// finishes or is cancelled.
    pub fn acquire_permit(&self) -> Arc<Semaphore> {
        Arc::clone(&self.permits)
    }
}

impl<T: Send + 'static> Clone for Queue<T> {
    fn clone(&self) -> Self {
        Queue {
            tx: self.tx.clone(),
            permits: Arc::clone(&self.permits),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_and_drain_in_order() {
        let (q, mut rx) = Queue::<u32>::new(1, 4);
        q.submit(1).unwrap();
        q.submit(2).unwrap();
        q.submit(3).unwrap();
        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(rx.recv().await, Some(2));
        assert_eq!(rx.recv().await, Some(3));
    }

    #[tokio::test]
    async fn submit_returns_full_when_at_depth() {
        let (q, _rx) = Queue::<u32>::new(1, 2);
        q.submit(1).unwrap();
        q.submit(2).unwrap();
        assert_eq!(q.submit(3), Err(SubmitError::QueueFull));
    }

    #[tokio::test]
    async fn submit_rejects_after_close() {
        let (q, rx) = Queue::<u32>::new(1, 2);
        drop(rx);
        assert_eq!(q.submit(1), Err(SubmitError::Closed));
    }

    #[tokio::test]
    async fn permit_blocks_concurrent_jobs() {
        let (q, _rx) = Queue::<u32>::new(1, 4);
        let p = q.acquire_permit();
        let _held = p.try_acquire().unwrap();
        // Second acquire from the same semaphore must fail because the
        // permit count is 1.
        assert!(p.try_acquire().is_err());
    }

    #[tokio::test]
    async fn multiple_active_permits_allow_concurrency() {
        let (q, _rx) = Queue::<u32>::new(3, 4);
        let p = q.acquire_permit();
        let h1 = p.try_acquire().unwrap();
        let h2 = p.try_acquire().unwrap();
        let h3 = p.try_acquire().unwrap();
        assert!(p.try_acquire().is_err());
        drop(h1);
        let _h4 = p.try_acquire().unwrap();
        drop((h2, h3));
    }
}
