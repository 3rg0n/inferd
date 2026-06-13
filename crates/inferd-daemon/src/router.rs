//! Backend router — priority-ordered policy + per-backend circuit
//! breaker.
//!
//! Per ADR 0007 §"Implementation shape":
//!
//! - **Policy** is priority-order. Backends are registered in the
//!   operator's preferred order (highest priority first); `dispatch`
//!   walks the list and returns the first backend that's both
//!   `ready()` and not currently circuit-broken.
//! - **Circuit breaker** is per-backend. After `failure_threshold`
//!   pre-stream or mid-stream failures within `failure_window`, the
//!   breaker opens and the backend is skipped for `cooldown` seconds.
//!   After cooldown the breaker is half-open: the next dispatch
//!   tries the backend again; success closes it, failure re-opens
//!   for another `cooldown`.
//! - **No retry, no failover**. `dispatch` returns one backend; the
//!   caller (lifecycle / lifecycle_v2) generates against it once
//!   and reports the outcome via `record_success` / `record_failure`.
//!   If the backend errors mid-stream the daemon emits one terminal
//!   error frame — it does *not* try a different backend (ADR 0007).
//!
//! Apps do not pick the backend (ADR 0006). There is no per-request
//! `backend` field on the wire.
//!
//! The `Router` itself is `Send + Sync` and shared via `Arc`. Internal
//! breaker state lives behind an `RwLock`; reads (the dispatch hot
//! path) take the read lock, and the only writers are the
//! `record_*` feedback hooks.

use inferd_engine::Backend;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Default failure threshold for the breaker. Modest — three
/// pre-stream failures in a window is enough to call a cloud
/// adapter hot, more is over-tolerance.
pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;

/// Default observation window for failures. A burst of three
/// 5xx responses inside 60s is unusual for a healthy upstream.
pub const DEFAULT_FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// Default cooldown after the breaker opens. Long enough that
/// a flapping cloud upstream doesn't get hammered, short enough
/// that an outage of <1 minute clears on its own.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

/// Tunables for the per-backend circuit breaker. The same policy
/// applies to every registered backend.
#[derive(Debug, Clone, Copy)]
pub struct BreakerPolicy {
    /// Failures within `failure_window` that open the breaker.
    pub failure_threshold: u32,
    /// Sliding window over which failures are counted.
    pub failure_window: Duration,
    /// Open-state duration. After this elapses, the breaker
    /// transitions to half-open on the next dispatch.
    pub cooldown: Duration,
}

impl Default for BreakerPolicy {
    fn default() -> Self {
        Self {
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            failure_window: DEFAULT_FAILURE_WINDOW,
            cooldown: DEFAULT_COOLDOWN,
        }
    }
}

/// Per-backend breaker state. Internal to the router.
#[derive(Debug, Clone)]
struct BreakerState {
    /// Timestamps of recent failures inside the window. Pruned on
    /// every read/write.
    failures: Vec<Instant>,
    /// `Some(when)` while the breaker is open; the value is the
    /// earliest dispatch instant at which we're willing to try the
    /// backend again (half-open).
    open_until: Option<Instant>,
}

impl BreakerState {
    fn new() -> Self {
        Self {
            failures: Vec::new(),
            open_until: None,
        }
    }

    /// Drop failures older than `window` from the count window.
    fn prune(&mut self, now: Instant, window: Duration) {
        let cutoff = now.checked_sub(window).unwrap_or(now);
        self.failures.retain(|&t| t >= cutoff);
    }

    /// `true` if the breaker is currently open at instant `now`.
    fn is_open(&self, now: Instant) -> bool {
        self.open_until.is_some_and(|until| now < until)
    }
}

/// A backend known to the router with its circuit-breaker slot.
struct Slot {
    backend: Arc<dyn Backend>,
    name: String,
    state: BreakerState,
}

/// Outcome of `Router::dispatch` — chosen backend plus the diagnostic
/// info the lifecycle layer needs to report success / failure later.
pub struct Dispatch {
    /// Chosen backend (use exactly once; report the outcome via
    /// `Router::record_success` / `record_failure`).
    pub backend: Arc<dyn Backend>,
    /// Stable name (`Backend::name()` snapshot — saves a vtable hop
    /// at outcome-recording time).
    pub name: String,
}

/// Errors returned by `Router::dispatch`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RouterError {
    /// No backends are registered. Configuration error.
    #[error("no backends registered")]
    NoBackends,
    /// Every registered backend is unavailable — either not ready
    /// or its circuit breaker is open. Surfaces to the caller as
    /// `code: backend_unavailable`.
    #[error("no backend available")]
    NoneAvailable,
}

/// Backend router. Holds the priority-ordered backend list and
/// per-backend breaker state. Cheap to clone via `Arc`.
pub struct Router {
    slots: RwLock<Vec<Slot>>,
    policy: BreakerPolicy,
    /// Map from `Backend::name()` → index into `slots`. The map is
    /// populated once at construction and never changes — backends
    /// aren't added or removed at runtime in v0.2 (no admin API for
    /// it). Keeps name-keyed feedback (`record_success` /
    /// `record_failure`) O(1) without a linear scan over the slot
    /// list.
    name_index: HashMap<String, usize>,
}

impl Router {
    /// Build a router from a priority-ordered list of backends.
    /// First entry is highest priority. Uses the default breaker
    /// policy.
    pub fn new(backends: Vec<Arc<dyn Backend>>) -> Self {
        Self::with_policy(backends, BreakerPolicy::default())
    }

    /// Build a router with explicit breaker tuning. Used by tests
    /// (short windows + cooldowns) and operator-tuned configs.
    pub fn with_policy(backends: Vec<Arc<dyn Backend>>, policy: BreakerPolicy) -> Self {
        let mut name_index = HashMap::with_capacity(backends.len());
        let slots: Vec<Slot> = backends
            .into_iter()
            .enumerate()
            .map(|(i, b)| {
                let name = b.name().to_string();
                name_index.insert(name.clone(), i);
                Slot {
                    backend: b,
                    name,
                    state: BreakerState::new(),
                }
            })
            .collect();
        Self {
            slots: RwLock::new(slots),
            policy,
            name_index,
        }
    }

    /// Number of registered backends. For diagnostics and tests.
    pub fn len(&self) -> usize {
        self.slots.read().expect("router rwlock poisoned").len()
    }

    /// `true` if no backends are registered.
    pub fn is_empty(&self) -> bool {
        self.slots
            .read()
            .expect("router rwlock poisoned")
            .is_empty()
    }

    /// Pick a backend for the next request.
    ///
    /// Walks the slot list in priority order and returns the first
    /// slot whose backend is `ready()` and whose breaker is not
    /// currently open. Slots whose breaker has timed out into the
    /// half-open state count as eligible — the next outcome reported
    /// for them either closes the breaker (`record_success`) or
    /// re-opens it (`record_failure`).
    pub fn dispatch(&self) -> Result<Dispatch, RouterError> {
        self.dispatch_filtered(|_| true)
    }

    /// Pick a backend for an embed request — same priority + breaker
    /// rules as `dispatch`, but skip slots whose backend doesn't
    /// advertise `capabilities().embed`. Without this filter, a
    /// multi-backend config that puts a generate-only backend ahead
    /// of an embed-capable one in the priority list would always
    /// land embed requests on the generate slot and yield
    /// `EmbedUnsupported`.
    pub fn dispatch_embed(&self) -> Result<Dispatch, RouterError> {
        self.dispatch_filtered(|backend| backend.capabilities().embed)
    }

    fn dispatch_filtered<F>(&self, predicate: F) -> Result<Dispatch, RouterError>
    where
        F: Fn(&Arc<dyn Backend>) -> bool,
    {
        let now = Instant::now();
        let mut guard = self.slots.write().expect("router rwlock poisoned");
        if guard.is_empty() {
            return Err(RouterError::NoBackends);
        }

        for slot in guard.iter_mut() {
            // Prune old failures so a stale window doesn't keep the
            // breaker open after a long quiet period.
            slot.state.prune(now, self.policy.failure_window);

            if !slot.backend.ready() {
                continue;
            }

            if !predicate(&slot.backend) {
                continue;
            }

            if slot.state.is_open(now) {
                continue;
            }
            // If the breaker had been open and the cooldown has
            // expired, transition to half-open by clearing the
            // `open_until` mark. The backend gets one shot; the
            // next outcome decides whether it stays closed.
            if slot.state.open_until.is_some() {
                slot.state.open_until = None;
                slot.state.failures.clear();
            }
            return Ok(Dispatch {
                backend: Arc::clone(&slot.backend),
                name: slot.name.clone(),
            });
        }

        Err(RouterError::NoneAvailable)
    }

    /// `true` once every registered backend reports ready. The
    /// lifecycle uses this to gate listener creation
    /// (THREAT_MODEL F-13). Breaker state is *not* consulted —
    /// readiness here is the boot-time engine-loaded check, not
    /// the per-request availability check.
    pub fn all_ready(&self) -> bool {
        let guard = self.slots.read().expect("router rwlock poisoned");
        !guard.is_empty() && guard.iter().all(|s| s.backend.ready())
    }

    /// Record a successful generation against the given backend.
    /// Closes the circuit breaker (clears the open mark and the
    /// failure window). Called by the lifecycle layer after a
    /// terminal `Done` frame for a request served by this backend.
    pub fn record_success(&self, name: &str) {
        let Some(&idx) = self.name_index.get(name) else {
            return;
        };
        let mut guard = self.slots.write().expect("router rwlock poisoned");
        if let Some(slot) = guard.get_mut(idx) {
            slot.state.failures.clear();
            slot.state.open_until = None;
        }
    }

    /// Record a failure against the given backend. Counts toward
    /// the failure window; opens the breaker once the threshold is
    /// reached. Called from both the pre-stream error path
    /// (`generate*` returned `GenerateError`) and the mid-stream
    /// error path (stream terminated without a `Done` frame).
    /// Both kinds of failure count equally per ADR 0007.
    pub fn record_failure(&self, name: &str) {
        let Some(&idx) = self.name_index.get(name) else {
            return;
        };
        let now = Instant::now();
        let mut guard = self.slots.write().expect("router rwlock poisoned");
        let Some(slot) = guard.get_mut(idx) else {
            return;
        };
        slot.state.prune(now, self.policy.failure_window);
        slot.state.failures.push(now);
        if slot.state.failures.len() as u32 >= self.policy.failure_threshold {
            slot.state.open_until = Some(now + self.policy.cooldown);
        }
    }

    /// `true` if the named backend's circuit breaker is currently
    /// open. Diagnostic surface; the dispatch path checks this
    /// internally and returns `NoneAvailable` if every backend is
    /// open.
    pub fn breaker_open(&self, name: &str) -> bool {
        let Some(&idx) = self.name_index.get(name) else {
            return false;
        };
        let now = Instant::now();
        let guard = self.slots.read().expect("router rwlock poisoned");
        guard.get(idx).is_some_and(|slot| slot.state.is_open(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_engine::mock::Mock;

    fn fast_policy() -> BreakerPolicy {
        BreakerPolicy {
            failure_threshold: 2,
            failure_window: Duration::from_millis(500),
            cooldown: Duration::from_millis(100),
        }
    }

    #[test]
    fn empty_router_dispatch_returns_no_backends() {
        let router = Router::new(vec![]);
        assert!(router.is_empty());
        assert_eq!(router.dispatch().err(), Some(RouterError::NoBackends));
    }

    #[test]
    fn dispatch_returns_ready_backend() {
        let mock = Arc::new(Mock::new());
        let router = Router::new(vec![mock.clone()]);
        let chosen = router.dispatch().expect("dispatch ok");
        assert_eq!(chosen.name, "mock");
        assert_eq!(chosen.backend.name(), "mock");
        assert!(router.all_ready());
    }

    #[test]
    fn unready_backend_returns_none_available() {
        let mock = Arc::new(Mock::new());
        mock.set_ready(false);
        let router = Router::new(vec![mock]);
        assert_eq!(router.dispatch().err(), Some(RouterError::NoneAvailable));
        assert!(!router.all_ready());
    }

    #[test]
    fn priority_picks_first_ready_backend() {
        // Two named backends — Mock currently always returns "mock"
        // for name(), so we use a thin wrapper to give them distinct
        // identities for the priority test.
        struct Named {
            inner: Mock,
            name: &'static str,
        }
        #[async_trait::async_trait]
        impl Backend for Named {
            fn name(&self) -> &str {
                self.name
            }
            fn ready(&self) -> bool {
                self.inner.ready()
            }
            async fn generate_v2(
                &self,
                req: inferd_proto::v2::ResolvedV2,
            ) -> Result<inferd_engine::TokenStreamV2, inferd_engine::GenerateError> {
                self.inner.generate_v2(req).await
            }
        }

        let high = Arc::new(Named {
            inner: Mock::new(),
            name: "high",
        });
        let low = Arc::new(Named {
            inner: Mock::new(),
            name: "low",
        });
        high.inner.set_ready(false);
        let router = Router::new(vec![high.clone(), low.clone()]);
        // High not ready → fall through to low.
        assert_eq!(router.dispatch().unwrap().name, "low");
        // High recovers → router prefers high again.
        high.inner.set_ready(true);
        assert_eq!(router.dispatch().unwrap().name, "high");
    }

    #[test]
    fn breaker_opens_after_threshold_failures() {
        let mock = Arc::new(Mock::new());
        let router = Router::with_policy(vec![mock], fast_policy());
        // First failure — still closed.
        router.record_failure("mock");
        assert!(!router.breaker_open("mock"));
        // Second failure → meets threshold (2), breaker opens.
        router.record_failure("mock");
        assert!(router.breaker_open("mock"));
        // Dispatch sees no available backends.
        assert_eq!(router.dispatch().err(), Some(RouterError::NoneAvailable));
    }

    #[test]
    fn success_resets_failure_count() {
        let mock = Arc::new(Mock::new());
        let router = Router::with_policy(vec![mock], fast_policy());
        router.record_failure("mock");
        router.record_success("mock");
        // Next failure restarts the count from zero — breaker stays
        // closed even though we've now had 2 lifetime failures.
        router.record_failure("mock");
        assert!(!router.breaker_open("mock"));
    }

    #[test]
    fn breaker_recovers_after_cooldown() {
        let mock = Arc::new(Mock::new());
        let router = Router::with_policy(vec![mock], fast_policy());
        router.record_failure("mock");
        router.record_failure("mock");
        assert!(router.breaker_open("mock"));
        // Wait past the cooldown.
        std::thread::sleep(Duration::from_millis(150));
        assert!(!router.breaker_open("mock"));
        // Dispatch puts the breaker into half-open and yields the
        // backend.
        assert_eq!(router.dispatch().unwrap().name, "mock");
    }

    #[test]
    fn old_failures_outside_window_dont_open_breaker() {
        let mock = Arc::new(Mock::new());
        let router = Router::with_policy(vec![mock], fast_policy());
        router.record_failure("mock");
        std::thread::sleep(Duration::from_millis(600)); // exceed the 500ms window
        router.record_failure("mock");
        assert!(!router.breaker_open("mock"));
    }

    #[test]
    fn record_failure_for_unknown_backend_is_a_noop() {
        let mock = Arc::new(Mock::new());
        let router = Router::new(vec![mock]);
        router.record_failure("does-not-exist"); // should not panic
        router.record_success("does-not-exist");
        assert!(!router.breaker_open("does-not-exist"));
    }

    #[test]
    fn dispatch_embed_skips_non_embed_backends() {
        // Wrapper that overrides `capabilities().embed` to false but
        // delegates everything else to Mock. Models the multi-backend
        // shape inferd ships in its first-boot default config: a
        // generate-only backend ahead of an embed-capable one in the
        // priority list.
        struct GenerateOnly {
            inner: Mock,
            name: &'static str,
        }
        #[async_trait::async_trait]
        impl Backend for GenerateOnly {
            fn name(&self) -> &str {
                self.name
            }
            fn ready(&self) -> bool {
                self.inner.ready()
            }
            fn capabilities(&self) -> inferd_engine::BackendCapabilities {
                inferd_engine::BackendCapabilities {
                    embed: false,
                    ..self.inner.capabilities()
                }
            }
            async fn generate_v2(
                &self,
                req: inferd_proto::v2::ResolvedV2,
            ) -> Result<inferd_engine::TokenStreamV2, inferd_engine::GenerateError> {
                self.inner.generate_v2(req).await
            }
        }

        let generate_only = Arc::new(GenerateOnly {
            inner: Mock::new(),
            name: "generate-only",
        });
        let embed_capable = Arc::new(Mock::new()); // mock advertises embed=true
        let router = Router::new(vec![generate_only.clone(), embed_capable.clone()]);

        // Plain dispatch picks the first ready backend regardless of
        // capability — that's the existing generate-path semantics.
        assert_eq!(router.dispatch().unwrap().name, "generate-only");

        // dispatch_embed must skip generate-only and land on the
        // embed-capable backend.
        let chosen = router.dispatch_embed().expect("embed dispatch ok");
        assert_eq!(chosen.name, "mock");
        assert!(chosen.backend.capabilities().embed);
    }

    #[test]
    fn dispatch_embed_returns_none_available_when_no_backend_supports_embed() {
        struct GenerateOnly {
            inner: Mock,
        }
        #[async_trait::async_trait]
        impl Backend for GenerateOnly {
            fn name(&self) -> &str {
                "generate-only"
            }
            fn ready(&self) -> bool {
                self.inner.ready()
            }
            fn capabilities(&self) -> inferd_engine::BackendCapabilities {
                inferd_engine::BackendCapabilities {
                    embed: false,
                    ..self.inner.capabilities()
                }
            }
            async fn generate_v2(
                &self,
                req: inferd_proto::v2::ResolvedV2,
            ) -> Result<inferd_engine::TokenStreamV2, inferd_engine::GenerateError> {
                self.inner.generate_v2(req).await
            }
        }

        let router = Router::new(vec![Arc::new(GenerateOnly { inner: Mock::new() })]);
        assert_eq!(
            router.dispatch_embed().err(),
            Some(RouterError::NoneAvailable)
        );
    }
}
