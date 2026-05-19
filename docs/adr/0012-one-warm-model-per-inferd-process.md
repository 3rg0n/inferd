# 0012. One warm model per inferd process

- Status: accepted
- Date: 2026-05-19

## Context

v0.1 ships with a single warm model held in memory at a time.
`docs/plan-v0.1.md` and `CLAUDE.md` both list "no multi-model
warm pool in v0.1" as a deliberate scope cut, with the implicit
promise that the question is open for v0.2+.

It is not. Punting it again is the failure mode the v0.1 → v0.1.0
punch list specifically warned against, and downstream work
(`inferdctl pull`, the router policy that lights up with the
second backend in v0.2, model-store documentation, the v0.2 cloud-
backend wiring) is gated on knowing the answer. ADRs are the
right place to make the call before that work commits to one
shape.

The decision space is binary:

**Option A** — One warm model per inferd process. Operators who
need N concurrent models run N inferd processes, each with its
own config, its own UDS / pipe / TCP endpoint, its own admission
queue.

**Option B** — Multi-model warm pool inside one daemon. The
daemon holds K models in memory simultaneously per a configured
budget; each inference request names which model to use; a
pool-aware admission queue serialises per-model; an eviction
policy (LRU, LFU, manual) reclaims memory when budget is
exceeded.

The choice has shape downstream. Picking it now is cheap;
picking it after the second backend lands is not.

## Decision

**Option A.** v0.1 (and the foreseeable v0.x cadence) commits to
one warm model per inferd process. Operators who need N
concurrent models run N inferd processes.

The wire protocol stays unchanged: `Request` does not gain a
`model` field. The model is process-level configuration, set
in `~/.inferd/config.json`, frozen for the lifetime of that
daemon. A request is implicitly addressed to the model that
process is serving.

The router (ADR 0007) continues to operate over backends, not
models. v0.2's cloud-backend work adds *backend* concurrency
(local-llamacpp + remote-OpenAI-compat under one daemon, picked
by router policy), not *model* concurrency. A daemon that wants
to expose `gemma-4-e4b` AND `claude-sonnet-4-6` is two daemons.

## Consequences

### Why this is the right shape

- **Matches the lean-core posture (ADR 0006).** A multi-model
  pool is product-shaped: eviction policy, memory budget,
  per-model admission queue, model-selection on the wire. None
  of it serves the "single local inference endpoint for the
  whole machine" mission; all of it is feature creep into the
  control-plane responsibilities that ADR 0006 explicitly says
  belong to ecosystem extensions.
- **Keeps the protocol untouched.** ADR 0008 froze v1 without a
  model-selection field. Adding one would either break v1 (forces
  v2) or add an optional field whose absence preserves v0.1
  semantics — the second creates two parallel codepaths in
  every consumer for the same notional capability. Cheaper to
  not have the capability.
- **Operators with multi-model needs already use multi-process
  patterns elsewhere.** It's how `redis` works (one DB per
  port-or-socket pair), how `postgres` works (one cluster per
  port), how `ollama` would work if its single-instance
  assumption could be inverted. Operators understand the
  pattern; the cost is one config file per model, not custom
  daemon work.
- **Memory accounting stays the operator's job, not the
  daemon's.** A multi-model pool with a memory budget has to
  decide what counts (mmap'd weights? KV cache? per-request
  buffers? OS page cache?) and reconcile against process RSS
  in a way that's portably correct on Linux/macOS/Windows.
  That's a non-trivial subsystem the daemon currently has zero
  of. One model per process moves that accounting to where it
  belongs: the operator's `systemd` / `launchd` / Windows
  Service definition, in terms the OS already exposes
  (`MemoryMax=`, `Mem.HardLimit`, job objects).

### What this costs

- **Operators who run 5+ models pay 5+ daemon overheads.**
  Each daemon is ~10 MB binary footprint plus tokio runtime
  plus admission queue plus log writers. For 5 models the
  overhead is ~50 MB. This is small compared to the model
  weights themselves (5 × 5 GB = 25 GB) but it is non-zero
  and worth naming.
- **Per-process config duplication.** Each daemon needs its
  own `~/.inferd/config-<model>.json`, its own socket /
  pipe / TCP endpoint, its own log directory. Mitigated by:
  the `--config` CLI flag already exists; per-process socket
  paths are trivial; ops convention will likely be a single
  `~/.inferd/` parent with per-model subdirectories.
- **Consumers that want to switch models without restarting
  cannot.** They have to disconnect from one inferd and
  connect to a different one (different socket / pipe / TCP
  port). For an interactive UI this is a UX papercut; for
  a long-running middleware it is a non-issue (the middleware
  picks its inferd at startup). Mitigated by: a future
  ecosystem-extension router process could expose a single
  endpoint that fans out to multiple inferds based on a
  request-level model field, without the daemon having to
  carry that complexity.

### What this explicitly does not change

- **The wire protocol stays frozen.** No `model` field added to
  `Request`. ADR 0008 unchanged.
- **The `Backend` trait stays unchanged.** It abstracts where a
  model is served (local FFI, remote HTTP), not which model.
  v0.2's cloud-backend work fits without modification.
- **The router (ADR 0007) stays unchanged.** It picks among
  registered backends per operator policy. A daemon serving
  `gemma-4-e4b` via router-policy "prefer local, fall back to
  Anthropic on circuit-break" still serves *one* model — the
  fallback is a different *backend* serving the same model.

### What this enables for `inferdctl`

The CLI (`inferdctl pull`, `inferdctl status`, `inferdctl
doctor`) operates on the shared CAS model store (ADR 0011), not
on a daemon. `inferdctl pull <name>` populates the store; the
daemon resolves a manifest at startup. This is the same shape as
`docker pull` / `docker run` — pull is detached from run. No
"pre-warm a pool slot" command is needed because there is no
pool.

### What this means for v0.2

When the second backend lands (OpenAI-compat per ADR 0007 +
external positioning), it will be in the same daemon process,
serving the same model name from a different upstream. Router
policy decides which backend handles each request (per ADR 0007
operator-config policy + circuit breakers). This is *backend*
multiplexing, not *model* multiplexing — it does not require
multi-model pool support.

A daemon serving multiple models would require a separate ADR
re-opening this decision and rewriting the `Backend` trait, the
admission queue, and the router. That work is not on any current
roadmap.

## Alternatives considered

- **Option B as described above.** Rejected on lean-core
  grounds (ADR 0006) and on the protocol-cost grounds (ADR
  0008). The use case it serves — interactive
  switch-without-restart — is better served by an
  ecosystem-extension router process exposing one endpoint per
  model name, fanning out to multiple inferd processes
  underneath. That router lives outside the daemon and can
  ship independently.
- **Hybrid: one warm + N cold-loadable models.** Daemon warms
  the configured default but accepts requests naming any model
  in `$MODELS_HOME` and lazy-loads on demand. Rejected: turns
  the daemon's lifecycle into a control plane (which model is
  warm right now? which is loading? what's the eviction
  policy?), which is exactly the responsibility ADR 0006 says
  lives in ecosystem extensions. Also re-opens the protocol
  question (consumers need a way to name models).
- **Defer the decision to v0.2.** Rejected: downstream work
  (`inferdctl` shape, v0.2 router design, model-store
  documentation, the system-unit deployment template's `Limit*`
  directives) is gated on knowing the answer now. Punting again
  is the failure mode the punch list specifically warned
  against.

## References

- ADR 0006 — lean-core posture (this is a direct application).
- ADR 0007 — backend routing semantics (unchanged by this).
- ADR 0008 — protocol v1 frozen; no model-selection field
  (this ADR is why).
- ADR 0011 — shared CAS model store (decoupled from daemon
  lifecycle, supports the operator-runs-multiple-daemons
  pattern this ADR commits to).
- `docs/plan-v0.1.md` §"Non-goals for v0.1" — the original
  punt this ADR closes.
