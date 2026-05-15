# 0007. Backend routing: operator policy, no in-daemon retry, no mid-stream failover

- Status: accepted
- Date: 2026-05-15

## Context

inferd is intended to host multiple `Backend` adapters
simultaneously — local engine via FFI (ADR 0005), and in v0.2 a
set of cloud adapters (Anthropic, OpenAI, Bedrock, Anthropic's
Bedrock variant, LiteLLM-compatible servers). The user-facing
value of running inferd, as opposed to integrating an inference
SDK directly into each app, is **transparent routing**: install
inferd plus credentials once, and every middleware on the
machine benefits — local model when offline, cloud model when
online and the operator prefers it, automatic re-routing as
backends become healthy or unhealthy.

This raises three coupled questions:

1. **Who picks the backend per request?** The calling app, or
   the daemon?
2. **What does the daemon do when its chosen backend errors
   *before* any tokens stream?**
3. **What does the daemon do when its chosen backend errors
   *during* a stream?**

The answers must respect the existing invariants in `context.md`,
particularly invariant #2: "fallback on error is the caller's
responsibility. Daemon reports cleanly; no retry/degrade/rewrite."

## Decision

**Q1 — who picks the backend.** The daemon picks. Apps do not
override. The wire protocol exposes no per-request backend field;
the operator configures a routing policy at daemon-config time.
This rule is also recorded in ADR 0006 ("apps do not pick the
backend; if they want that, they should write their own
provider SDK integration").

**Q2 — pre-stream failure.** No in-daemon retry. If the chosen
backend errors before the first token, the daemon emits one
`error` frame to the caller and the request is over. The caller
decides whether to retry. On retry, the policy re-evaluates and
may pick a different backend (because the failed one tripped a
circuit breaker), so a caller's idempotent retry naturally
benefits from updated policy state without inferd having
"retried" anything.

**Q3 — mid-stream failure.** No mid-stream failover, ever. If
the chosen backend errors after the first token, the daemon
emits one `error` frame (with the partial state observable to
the caller via the tokens already streamed) and the request
ends. The caller decides whether to retry, fully aware that a
retry will produce a fresh independent generation.

The daemon's only stateful contribution to routing is a
**circuit breaker** per backend: N failures within a window
mark the backend cold for K seconds; cold backends are skipped
by the policy. This is policy *evolution*, not retry.

## Implementation shape

```rust
// Sketch — see crates/inferd-daemon/src/router.rs at M2/M5.

pub trait Backend { /* unchanged from inferd-engine */ }

pub struct Router {
    backends: Vec<Arc<dyn Backend>>,
    policy:   Policy,
    breakers: HashMap<&'static str, CircuitBreaker>,
}

impl Router {
    pub async fn dispatch(&self, req: Request) -> Result<TokenStream> {
        let backend = self.policy.choose(&self.backends, &self.breakers)?;
        match backend.generate(req).await {
            Ok(stream) => Ok(stream),
            Err(e)     => {
                self.breakers.get_mut(backend.name()).record_failure();
                Err(e)  // caller sees one error frame; no retry here
            }
        }
    }
}
```

**v0.1 router**: no-op. One backend (`llamacpp`), trivial
policy ("use the only backend"), no circuit breaker yet (single
backend has nothing to fail over to). The shape is in place;
the second backend doesn't exist.

**v0.2 router**: real policy + circuit breaker, gated on the
first cloud `Backend` adapter landing.

## Consequences

**Why this is right:**

- Preserves invariant #2 verbatim. The daemon never retries.
  Callers retry. The daemon's circuit-breaker state evolves
  *between* requests, which is policy, not retry.
- Mid-stream failover is structurally impossible to do
  correctly. The hidden state of a transformer is the *KV
  cache of that specific model*; another model cannot
  continue from it. Even "re-prompt the second model from
  the start" produces visible duplication in the user's
  output. We rejected the idea outright.
- "App sends inference request, app gets tokens, app does not
  know or care which provider served it" matches the user's
  vision. Operator installs Anthropic credentials once,
  every middleware benefits.
- Apps that want fine-grained per-call control (specific
  model version, custom timeouts, vendor-specific features)
  are explicitly told: write your own provider SDK
  integration into your app. inferd is not for that workload.
- The `Backend` trait is unchanged. Routing is a daemon-level
  concern that wraps the trait, not a trait change.

**What we take on:**

- The daemon now has *real* state — the circuit breaker. It
  needs tests for failure-counting windows, half-open
  recovery, and policy interaction.
- "Which backend served this request?" is a question debug-
  curious operators will ask. We answer it by including the
  serving backend's `name()` in the `done` frame's metadata.
  (This requires a minor wire-additive: the v1 frame already
  has room for it via response-frame fields; verify against
  the spec before M2.)
- Operator complexity. A bad policy DSL or bad defaults will
  bite. v0.1 ships with a single backend so this risk is
  deferred; v0.2 must ship sane defaults and clear docs.
- A clear UX gap: callers retrying immediately on error may
  hammer a flapping cloud backend. Mitigated by the circuit
  breaker — after a few failures the policy stops choosing
  the cloud backend for a cooldown window. Callers' retries
  then route locally without coordination.

**What we explicitly reject:**

- **In-daemon retry on a different backend before any token
  streams** (variant A3 in the design discussion). The
  apparent UX win — "anthropic 503'd in <100ms, local served
  you and you never knew" — is small; the cost is doubling
  the failure-handling surface, hiding error rates from
  callers, and adding a place where caller retries can
  multiply with daemon retries. Net: not worth it.
- **Mid-stream failover.** Already covered above.
  Structurally broken regardless of implementation.
- **Caller-controlled backend selection on the wire.**
  Already covered in ADR 0006.

## What apps must do

This ADR places real obligations on calling apps. They are
documented here so middleware authors can find them:

- Apps must be **idempotent in the operational sense**: a
  retried inference request must be safe to issue. The model
  may produce different output (sampling is non-deterministic
  at temperature > 0), but submitting the same `Request`
  twice must not corrupt app state.
- Apps must **handle the `error` frame**. The contract says
  the daemon emits exactly one terminal frame per request id
  — `done` or `error` — and apps must treat any other
  termination (EOF without a terminal frame) as `error`.
- Apps must **own retry policy**. The daemon does not retry.
  If an app wants exponential backoff, jitter, or a
  retry-budget, it implements that itself.
- Apps must **not assume a specific backend served them**.
  The serving backend is exposed in the `done` frame
  metadata for diagnostic purposes only. App logic must not
  branch on backend identity.

## Alternatives considered

- **Variant A1 — strict caller choice.** Caller specifies
  backend in the request. Rejected: defeats the entire point
  of inferd-as-routing-infrastructure and leaks credentials
  into every app.
- **Variant A2 — operator policy, no retry.** This is what
  the ADR adopts.
- **Variant A3 — operator policy, single in-daemon retry on
  different backend before any token streams.** Rejected:
  see "What we explicitly reject" above.
- **Variant B — mid-stream failover.** Rejected as
  structurally broken.

## References

- `context.md` invariant #2 — "fallback on error is the
  caller's responsibility."
- ADR 0001 — wire protocol frozen; this ADR introduces no
  v1 fields.
- ADR 0006 — lean-core posture; apps-do-not-pick-the-backend
  is also documented there.
- `docs/protocol-v1.md` §"Response stream" — the
  `done`/`error` terminal-frame guarantee that callers
  rely on.
