# 0006. Lean core, ecosystem extensions live as separate processes

- Status: accepted
- Date: 2026-05-15

## Context

A serving daemon attracts feature requests like a magnet:

- "Can it expose an HTTP endpoint?"
- "Can it speak the OpenAI-compat API?"
- "Can it serve a web chat UI?"
- "Can it embed a Prometheus exporter?"
- "Can it act as a Slack bot?"
- "Can my app override the backend per-request?"

Each one sounds reasonable in isolation. Together they bloat the
daemon into a kitchen-sink server with a wide attack surface, slow
build, and an unclear identity.

The reframe driving this ADR — captured in
`docs/ai.internals.explained.md` and ADR 0005 — is that inferd is
infrastructure, not a product. Linux-kernel posture: ship a small
core, expose a clean interface, let the ecosystem build the
extensions. The kernel does not include a web browser; the kernel
does not include curl. They are *consumers* of the kernel.

## Decision

The inferd daemon ships **only** these capabilities:

1. NDJSON-over-IPC transport (Unix socket, Windows named pipe,
   loopback TCP) — see `docs/protocol-v1.md`.
2. The `Backend` trait + a small set of in-tree adapters chosen
   by the inferd maintainers (v0.1: local llama.cpp via FFI per
   ADR 0005; v0.2: Anthropic, OpenAI, Bedrock, etc.).
3. Operator-configured routing policy across the registered
   backends — see ADR 0007.
4. Admission queue, single-instance lock, activity log, security
   perimeter (per `context.md` invariants).

The daemon ships **none** of the following. They live as separate
processes / projects that consume inferd over IPC:

- HTTP transport. If a tool needs HTTP-on-localhost, install
  inferd plus a tiny `inferd-http` adapter process that does
  HTTP-in / NDJSON-out and forwards to the inferd socket.
- OpenAI-compatible REST surface. Same pattern — separate
  adapter process.
- gRPC, GraphQL, server-sent events, websockets. Same pattern.
- Web UIs, chat interfaces, dashboards.
- Prometheus or OpenTelemetry exporters as built-in features
  (the activity log is NDJSON; a separate process can scrape
  and translate it).
- Per-request backend override by the calling app.

That last one is worth naming explicitly because it is the most
common feature creep request and the most damaging to the design.
**Apps do not pick the backend.** They send a `Request`, they
receive tokens. If an app wants direct, app-specific control over
which provider serves an inference — pick a model version, set
custom timeouts, pin a region — that app is asking for an SDK
integration, not a serving endpoint. The right answer is "go
write your own Anthropic / OpenAI / Bedrock client; integrate it
into your app directly." inferd's value proposition is the
opposite: *one* warm endpoint on the machine, transparent backend
selection, zero per-app credential plumbing. Apps that don't want
that are not inferd's audience for that workload.

The `Backend` trait set is curated. Adding a new in-tree adapter
requires an ADR and a maintainer. Out-of-tree adapters can exist
as third-party crates that implement `Backend`, but they are not
shipped with the daemon.

## Consequences

**Why this is the right call:**

- The daemon stays small and auditable. Fewer dependencies,
  fewer CVEs, faster builds, smaller binary, smaller attack
  surface.
- The 15-component inference stack
  (`docs/ai.internals.explained.md`) splits cleanly:
  components 1–10 are the engine (vendored), 11–12 + 14–15 are
  the daemon's perimeter, 13 (transport) is intentionally
  minimal. Higher-level surfaces are someone else's process.
- Extensions can iterate at their own pace without daemon
  releases. A bug in `inferd-http` does not require a
  redeploy of the inference daemon.
- The daemon's threat model is bounded. We do not have to
  threat-model HTTP request smuggling, OpenAI-compat parser
  edge cases, or web-UI XSS — those are someone else's problem
  in someone else's process.
- The contract is a stable wire protocol, not a Rust API. Any
  language can write an extension.

**What we take on:**

- A real ecosystem story. The first time someone wants HTTP, we
  point them at a reference `inferd-http` adapter (which we may
  ship in `clients/` or as a sister repo) and a one-paragraph
  HOWTO. Without that, "just write a separate process" reads
  as gatekeeping.
- Discoverability. Extensions live in different repos / crates;
  the inferd README must list known ones.
- A higher bar for in-tree adapters. Every cloud `Backend`
  added to the daemon is operator-visible policy surface, not
  an isolated extension. Curate accordingly.

**What we explicitly reject:**

- Any in-daemon HTTP server. Not optional. Not behind a
  feature flag. If a future contributor proposes one, this ADR
  is the answer; if the design has materially shifted, write a
  superseding ADR first.
- Per-request app-level backend override on the wire. The v1
  protocol has no such field; v2 (separate socket per ADR
  0001) will not add one either, unless a future ADR
  re-litigates this decision with concrete evidence the
  current routing model is insufficient.

## Pattern for ecosystem adapters

The reference pattern (which we will document and demo with at
least one adapter — likely an HTTP one — before v0.1 GA):

```
┌──────────────┐  HTTP/JSON   ┌─────────────┐  NDJSON/IPC  ┌─────────┐
│  Calling app │─────────────▶│ inferd-http │─────────────▶│ inferd  │
└──────────────┘              └─────────────┘              └─────────┘
```

`inferd-http` is a tiny separate binary. It implements whatever
HTTP shape is desired (OpenAI-compat, custom JSON, whatever),
opens a connection to the inferd socket, and forwards. It can be
written in any language. It can be installed independently. It
can crash without taking inferd with it.

This is the linux-kernel pattern: kernel exposes syscalls; user
space builds shells, browsers, and editors on top.

## Alternatives considered

- **Bundle HTTP behind a feature flag.** Rejected. Feature flags
  drift; one operator's "off" is another's "on by default in
  the package manager build." A capability that exists in the
  binary will eventually be invoked. The cleanest "no HTTP" is
  "the code does not exist."
- **Provide a plugin loader for in-process adapters.** Rejected
  for v0.1 — substantial complexity (ABI stability, sandboxing,
  crash isolation) for a problem already solved by separate
  processes talking NDJSON.
- **One binary with subcommands** (`inferd serve`,
  `inferd-http serve`). Tempting because of single-binary
  shipping, but it conflates extension surface with daemon
  surface in CI, threat model, and release cadence. Separate
  binaries from separate repos is the cleaner cut.

## References

- `docs/ai.internals.explained.md` §"What 'owning the stack'
  actually means" — the level-of-ownership framing this ADR
  operationalises.
- ADR 0005 — engine consumed as a library, not a subprocess.
  Same posture, applied at the engine layer.
- ADR 0007 — routing policy lives inside the daemon; the rule
  that apps don't pick the backend is enforced there.
