//! Backend router.
//!
//! Per ADR 0007: the daemon picks the backend per request based on
//! operator-configured policy. v0.1 ships a **no-op router** with a single
//! registered backend, so `dispatch` always returns it. The shape (`Router`,
//! choose-fn, breaker map) is in place so v0.2 can add a real policy +
//! circuit breaker without restructuring the daemon.
//!
//! Apps do not pick the backend (ADR 0006). There is no per-request
//! `backend` field on the wire.

use inferd_engine::Backend;
use std::sync::Arc;

/// A backend known to the router. v0.1 only ever holds one of these.
pub struct Router {
    backends: Vec<Arc<dyn Backend>>,
}

/// Errors returned by `Router::dispatch`.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// No backends are registered. Configuration error.
    #[error("no backends registered")]
    NoBackends,
    /// All registered backends are unavailable (e.g. all circuit-broken in
    /// v0.2). Surfaces to the caller as `code: backend_unavailable`.
    #[error("no backend available")]
    NoneAvailable,
}

impl Router {
    /// Build a router with one or more backends. Order matters in v0.2
    /// (priority-ordered policy); in v0.1 only the first backend is used.
    pub fn new(backends: Vec<Arc<dyn Backend>>) -> Self {
        Self { backends }
    }

    /// Number of registered backends. For diagnostics and tests.
    pub fn len(&self) -> usize {
        self.backends.len()
    }

    /// `true` if no backends are registered.
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    /// Pick a backend for a request.
    ///
    /// v0.1: returns the first backend if it is `ready()`; otherwise
    /// returns `NoneAvailable`. v0.2 replaces this with a policy+breaker
    /// implementation.
    pub fn dispatch(&self) -> Result<Arc<dyn Backend>, RouterError> {
        if self.backends.is_empty() {
            return Err(RouterError::NoBackends);
        }
        let b = &self.backends[0];
        if !b.ready() {
            return Err(RouterError::NoneAvailable);
        }
        Ok(Arc::clone(b))
    }

    /// `true` once every registered backend reports ready. The lifecycle
    /// uses this to gate listener creation (THREAT_MODEL F-13).
    pub fn all_ready(&self) -> bool {
        !self.backends.is_empty() && self.backends.iter().all(|b| b.ready())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_engine::mock::Mock;

    #[test]
    fn empty_router_dispatch_returns_no_backends() {
        let router = Router::new(vec![]);
        assert!(router.is_empty());
        match router.dispatch() {
            Err(RouterError::NoBackends) => {}
            other => panic!("expected NoBackends, got {:?}", other.err()),
        }
    }

    #[test]
    fn dispatch_returns_ready_backend() {
        let mock = Arc::new(Mock::new());
        let router = Router::new(vec![mock.clone()]);
        let chosen = router.dispatch().expect("dispatch ok");
        assert_eq!(chosen.name(), "mock");
        assert!(router.all_ready());
    }

    #[test]
    fn unready_backend_returns_none_available() {
        let mock = Arc::new(Mock::new());
        mock.set_ready(false);
        let router = Router::new(vec![mock]);
        match router.dispatch() {
            Err(RouterError::NoneAvailable) => {}
            other => panic!("expected NoneAvailable, got {:?}", other.err()),
        }
        assert!(!router.all_ready());
    }
}
