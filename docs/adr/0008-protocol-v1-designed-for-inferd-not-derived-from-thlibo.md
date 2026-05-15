# 0008. Protocol v1 designed for inferd, not derived from thlibo

- Status: accepted
- Date: 2026-05-15
- Supersedes: [0001](0001-wire-protocol-inherited-from-thlibo.md)

## Context

ADR 0001 froze the v1 wire protocol to be byte-for-byte
compatible with thlibo's existing NDJSON IPC, on the reasoning
that "thlibo v0.2 migration is an import swap, not a
marshalling rewrite." That reasoning treated thlibo as the
fixed point and inferd as the dependent.

The roles have inverted. Going forward inferd is the
infrastructure and thlibo is one of its consumers. The
maintainer of both projects has confirmed that thlibo will be
refactored to match whatever envelope inferd designs — not the
other way around.

This frees the v1 design from a constraint that was never
really paying for itself. thlibo's protocol was written
incrementally inside a Go daemon; carrying its quirks forward
into a from-scratch Rust design (e.g. omitting a `stop_reason`
field, conflating `text` and `content`, not exposing which
backend served a request) costs us clarity for no migration
benefit.

## Decision

The inferd v1 wire protocol is designed *for inferd*, on its
own merits. thlibo is treated as one client and will be
updated by its maintainer to match. Specifically:

- The `Response` frame schema may diverge from thlibo's where a
  cleaner design exists. Concretely, v1 adds:
  - `stop_reason` on `done` frames (`end | length | cancelled | error`).
  - `backend` on `done` frames (diagnostic — names the
    `Backend::name()` that served the request, per ADR 0007).
  - `code` on `error` frames (machine-readable enum:
    `queue_full | backend_unavailable | invalid_request | internal`).
- The `Request` shape stays close to thlibo's (no value in
  changing it for its own sake): `id`, `messages`, sampling
  params with documented defaults, `image_token_budget`,
  `grammar`. Multimodal content arrays are not in v1.
- "Byte-compatible with thlibo" is no longer a goal of v1.
  Schema-level compatibility is the only thing thlibo v0.2
  needs, and thlibo will be refactored to consume the inferd
  Go client crate (M5 deliverable).

## Consequences

**Why this is right:**

- `stop_reason` lets clients distinguish "the model finished
  cleanly" from "we hit `max_tokens`" from "the user
  disconnected" without parsing prose. This is useful enough
  that omitting it was always a deficit, not a stylistic
  choice.
- `backend` on `done` operationalises ADR 0007's observability
  requirement (which backend served this request?) without
  adding a separate query path.
- `code` on `error` lets callers branch retry policy on error
  class — `queue_full` is "retry now," `backend_unavailable`
  is "retry with backoff," `invalid_request` is "don't retry,
  fix your input."
- The protocol is now a clean reference document, not a
  derivation. Future contributors do not have to read thlibo's
  Go source to disambiguate field semantics.

**What we take on:**

- thlibo's v0.1 → v0.2 work now includes a real protocol
  refactor in addition to deleting `internal/daemon/`. That is
  the maintainer's stated intent, so this is not a surprise
  cost — but it is a cost.
- The "drop-in replacement" framing in `context.md` and the v0.1
  plan needs softening. inferd is still a *functional*
  replacement for thlibod; it is just no longer a wire-bytes
  replacement.

**What stays the same:**

- The transport (NDJSON over UDS / named pipe / loopback TCP),
  the framing rules (one JSON object per line, 64 MiB cap), the
  admission semantics (1 active + 10 queued, non-blocking
  submit, cancel-on-disconnect), the ready gating, and the
  per-caller identity model are all unchanged.
- The "v2 goes on a separate socket path, no in-band
  negotiation" rule from ADR 0001 is preserved here. v1 stays
  immutable once shipped; breaking changes are v2 on a
  separate endpoint.

## Alternatives considered

- **Keep ADR 0001.** Rejected. The maintainer of both projects
  confirmed thlibo will follow inferd, removing the constraint's
  rationale.
- **Bigger redesign — versioned envelope, capability
  exchange, etc.** Rejected for v1. The current shape is
  adequate; we do not have evidence yet of what would justify
  a bigger break. v2 can carry that load if it ever
  materialises.
- **Add only `stop_reason`, leave `backend` and `code` for
  v2.** Rejected. The window for clean-slate additions is
  open right now; closing it for two of three obvious wins
  has no benefit.

## References

- `docs/protocol-v1.md` — the rewritten spec.
- ADR 0007 — `backend` field on `done` frames operationalises
  routing observability.
- ADR 0001 — superseded; flipped to `superseded by 0008`.
