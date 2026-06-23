# 0022. No inbound network listener in the daemon — deprecate loopback TCP

- Status: accepted
- Date: 2026-06-23

## Context

The daemon has shipped an opt-in inbound loopback-TCP listener since the
pre-M1 work (`--tcp`, API-key-gated per [ADR 0009](0009-pre-m1-open-questions-resolved.md)
§"Loopback TCP", THREAT_MODEL F-8). Its only genuine user-facing purpose
was cross-OS reachability: a Windows named pipe is not reachable from
inside WSL (nor a Linux UDS from the Windows host), so a localhost TCP
port was the stopgap answer to issue #88. Its other use is test
ergonomics — TCP is the one transport that behaves identically on Linux,
macOS, and Windows CI runners, so the Go client tests and the W1
cross-language round-trip gate launch the daemon with `--tcp`.

Neither reason justifies a network listener in the daemon:

- **Cross-OS reach is already reassigned.** [ADR 0020](0020-inferd-http-bridge-is-a-separate-process.md)
  defines a separate `inferd-http` bridge whose **Surface B** is exactly
  "native inferd frames over the network … tunneled over a localhost TCP
  port," so first-party middleware dials a port on WSL instead of a pipe
  on Windows. ADR 0020 left an explicit open question — should the
  daemon's existing TCP serve Surface B (option a), or should TCP be
  re-homed into the bridge (option b)? This ADR resolves it: **option
  (b)**. The daemon does not solve cross-OS reachability; the bridge
  does.
- **Test ergonomics is not a production requirement.** "Our harness is
  simpler with TCP" is a reason that serves the tests, not users, and is
  not grounds for shipping a network listener.

A UDS / named pipe is authenticated by kernel-attested peer credentials
(UID / SID, THREAT_MODEL F-7): the OS *proves* who connected. Loopback
TCP cannot, so it falls back to a shared pre-shared API key (F-8) — a
weaker, bolt-on auth path that exists *only because* TCP exists. Removing
inbound TCP deletes an entire weaker auth surface and leaves one
authentication model instead of two. inferd's identity is local-only,
peer-cred-authenticated IPC; an inbound network listener contradicts it.

## Decision

**The daemon binds no inbound network listener of any kind — not even on
loopback.** Its IPC surface is Unix domain socket (Unix) and named pipe
(Windows) only. Anything that needs to reach inferd over a network port
is the job of the separate `inferd-http` bridge process (ADR 0020,
Surface B), which holds no model and is a consumer of `inferd-client`
like any other middleware.

Outbound TCP/HTTPS is unaffected: the cloud backend adapters
(`openai-compat`, `bedrock-invoke`) and the narrow ADR 0010
model-bootstrap fetch make *outbound* connections and are out of scope
here. This ADR concerns only *inbound* listeners.

**Phased removal to protect the v0.4.0 GA gate:**

- **v0.4.0 — deprecated, unreferenced.** The `--tcp` flag, the
  first-frame `{"type":"auth","key":...}` path, and the client
  `dial_tcp` / `DialTCP` constructors **remain in the code** so the W1
  cross-language round-trip gate and the Go/Rust client tests keep
  working without a transport rewrite right before the tag. They are
  removed from **all user-facing surfaces**: the wire spec
  (`docs/protocol-v2.md`), the `inferd-client` and `clients/go` READMEs,
  the GitHub Pages site, and the sample-client quickstarts. The client
  constructors carry a deprecation note pointing at UDS / pipe.
- **v0.4.1 — removed.** `--tcp` (and `INFERD_TCP`), the first-frame TCP
  auth, the `--api-key` flag's daemon role, `dial_tcp` / `DialTCP`, and
  the constant-time key compare are deleted. The client tests move to
  UDS (Unix) / named pipe (Windows). Tracked as its own task.

This **supersedes the "Loopback TCP" clause of ADR 0009** (the
TCP-identity-reduces-to-API-key decision) and **resolves the open
question in ADR 0020** in favour of option (b).

## Consequences

**Easier:**
- One authentication model (kernel peer credentials), one trust story.
  The weaker shared-key TCP path goes away.
- The daemon's "local-only IPC" claim becomes literally true and
  enforceable — the GitHub Pages "what it isn't" list gains a concrete
  "no network listener, even loopback" entry.
- ADR 0020's bridge gets a clean mandate: it owns *all* network surfaces
  (OpenAI-compat HTTP **and** native-over-TCP), with one auth/TLS path.

**Harder:**
- The client test harness must, at v0.4.1, branch by OS (UDS vs named
  pipe) instead of using one uniform TCP transport. This is the cost
  ADR 0020 option (b) always implied.
- Cross-OS (WSL ↔ Windows) reachability is unavailable until the
  `inferd-http` bridge is built. Until then, co-located middleware uses
  the native UDS / pipe; cross-boundary callers wait for the bridge.

## References

- ADR 0006 — lean core; network/transport surfaces live as separate
  processes, never in the daemon.
- ADR 0009 — pre-M1 decisions; its "Loopback TCP" clause is superseded
  here.
- ADR 0010 / invariant #11 — the daemon's only outbound HTTPS is model
  bootstrap; no inbound HTTP server.
- ADR 0014 / ADR 0018 — the CLI and the bridge are reference
  middlewares, not privileged surfaces.
- ADR 0020 — the `inferd-http` bridge; this ADR resolves its open
  question (option b) and gives Surface B its home.
- Issue #88 — WSL ↔ Windows cross-OS reachability (now a bridge concern).
