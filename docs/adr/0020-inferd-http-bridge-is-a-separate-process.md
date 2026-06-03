# 0020. The HTTP/OpenAI-compat bridge is a separate process, not in the daemon

- Status: accepted
- Date: 2026-06-03

## Context

Two consumer needs have surfaced that both want inferd reachable over
HTTP rather than the daemon's native NDJSON-over-IPC (UDS / named pipe):

1. **OpenAI-SDK tools** (OpenCode, Continue, and other editors/agents
   that speak `/v1/chat/completions` + `/v1/embeddings`) want to point
   at a local inference endpoint unchanged. They will not learn
   inferd's native frame schema.

2. **First-party middleware that runs on both Windows and WSL.** The
   author wants to write inferd's *native* schema once and not switch
   transports/schemas when the same code runs inside WSL versus on the
   Windows host. Today the native surface is UDS on Linux and a named
   pipe on Windows; crossing the WSL↔Windows boundary is awkward
   (issue #88).

The tempting shortcut — add an HTTP listener (and an OpenAI-compat
translation layer) to `inferd-daemon` — is **forbidden by ADR 0006**
(lean core: HTTP / OpenAI-compat / web UI / gRPC live as separate
processes, never in the daemon) and by invariant #11 (the daemon's only
outbound HTTPS is the narrow ADR 0010 model-bootstrap carve-out; "no
HTTP server … no OpenAI-compat … no HTTP after ready"). ADR 0017 already
named the intended shape in passing: *"`/v1/embeddings`-shaped HTTP is
an ecosystem-extension job (an `inferd-http` adapter process, mirroring
the OpenAI-compat pattern), not a daemon job."*

## Decision

Build a **separate binary** (working name `inferd-http`) that talks
NDJSON-over-IPC to the daemon via the existing `inferd-client` library —
the same client every other consumer uses. The daemon is unchanged; it
gains no HTTP surface. The bridge exposes **two surfaces**:

- **Surface A — OpenAI-compat HTTP.** `/v1/chat/completions` and
  `/v1/embeddings` (streaming + non-streaming). The bridge translates
  OpenAI request/response JSON ↔ inferd v1/v2/embed frames. This is the
  endpoint OpenCode and other OpenAI-SDK tools point at.

- **Surface B — native inferd frames over the network.** The exact
  v1/v2/embed NDJSON schema the daemon speaks on IPC, tunneled over a
  localhost TCP port. First-party middleware writes inferd's native
  schema once and dials a port on WSL instead of a pipe on Windows —
  same JSON either side of the boundary, no per-OS branching.

The bridge is a consumer, not a privileged surface (same posture as the
`inferdctl` CLI per ADR 0014). It holds no model and does no inference;
it is pure protocol translation + transport. Auth (token / API key) and
TLS terminate at the bridge, not the daemon.

## Consequences

**Why this is right:**
- Keeps ADR 0006 / invariant #11 intact — the daemon stays HTTP-free
  and lean. The bridge can be packaged, versioned, and shipped (or not)
  independently.
- One auth'd network endpoint to point tools at, for both the
  OpenAI-compat and native cases.
- Editors get inferd "for free" via the surface they already speak;
  first-party middleware gets schema stability across the WSL boundary.

**What it costs:**
- A new process to run, configure, and secure (its own TLS/token story).
- OpenAI-compat translation is real, ongoing surface area (tool-call
  shape, streaming deltas, embeddings vs. chat, model-name mapping).
- The bridge must track inferd's frozen wire surfaces (v1/v2/embed) the
  way any consumer does.

**Open question (resolve at implementation time):** the daemon already
has opt-in loopback-TCP for the native protocol (`--tcp` / `--v2-tcp` /
`--embed-tcp`, API-key-gated). Surface B could either (a) be served by
the daemon's existing TCP, with the bridge providing only Surface A; or
(b) be re-homed into the bridge so there is a single network surface
with one auth path. Leaning toward (b) for a single point of
configuration, but (a) is less code. Decide when the bridge work starts.

## Scope / timing

Implementation is deferred until **after v0.3.0 stable** (v0.3 is
runtime accelerator detection, ADR 0019). This ADR records the shape so
the decision isn't re-litigated; the build is a separate
ecosystem-extension project tracked in its own issue.

## References

- ADR 0006 — lean core; HTTP/OpenAI-compat live as separate processes.
- ADR 0010 / invariant #11 — the daemon's only outbound HTTPS is model
  bootstrap; no HTTP server in the daemon.
- ADR 0014 — the CLI (and by extension this bridge) is a reference
  middleware, not a privileged surface.
- ADR 0017 — names the `inferd-http` adapter-process shape.
- Issue #88 — WSL ↔ Windows cross-OS reachability (Surface B motivation).
