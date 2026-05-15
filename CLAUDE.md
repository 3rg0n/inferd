# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

**Planning stage.** No Rust sources exist yet — the repo is a design brief plus scaffolded crate directories. Treat `docs/plan-v0.1.md`, `context.md`, `docs/protocol-v1.md`, and `docs/adr/` as the authoritative spec. Workspace members in `Cargo.toml` are intentionally commented out until each crate is scaffolded (milestone M1+).

Before writing code, read `context.md` — it is the hand-off brief for the first implementer and names the invariants ported from the sibling project.

## Sibling project: thlibo

inferd is a ground-up Rust port of the embedded `thlibod` daemon inside [`3rg0n/thlibo`](https://github.com/3rg0n/thlibo) (Go). When a design question is ambiguous, the answer is almost certainly in thlibo's source:

- `internal/daemon/lifecycle.go` — boot/accept/dispatch/shutdown loop being ported to `inferd-daemon/src/lifecycle.rs`
- `internal/daemon/engine.go` — llamafile subprocess supervisor → `inferd-engine/src/llamafile.rs`
- `internal/ipc/protocol.go` — authoritative NDJSON frame shape for the wire protocol
- `internal/queue/` — fixed-depth admission queue semantics
- `internal/logx/` — NDJSON activity log record shape (keep field names identical so ops dashboards span both projects)
- `THREAT_MODEL.md` — every finding L2/L4/L5/L6 applies here; port the remediation, not just the feature

Clone thlibo alongside this repo. Semantics are copy-worthy; the Go types are not — re-express idiomatically in Rust.

## Wire protocol is frozen

Protocol v1 is designed for inferd on its own merits ([ADR 0008](docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md), supersedes 0001). It is **immutable once shipped**. Do not change framing, rename fields, or break existing field semantics. Breaking changes become v2 on a **separate socket path** — no in-band version negotiation. See `docs/protocol-v1.md`.

Frame cap: 64 MiB per line. Use a bounded reader, not an auto-growing buffer.

Backwards-additive changes within v1 (new optional fields older servers MUST ignore and older clients MUST NOT require) are acceptable; v0.1 enforces "unknown fields ignored on parse" so the door for additive changes stays open.

## Model reference material

`docs/` also contains upstream Gemma 4 reference docs — not inferd design, but context on the model the llamafile backend serves:

- `run-gemma-content-generation-and-inferences.md` — framework/variant landscape
- `text.function.calling.with.gemma.4.md` — tool-use schema (potential v0.2+ protocol extension)
- `thinking.mode.in.gemma.md` — reasoning trace separation (potential v0.2+ response-frame extension)

Treat these as background, not requirements. v0.1 does not expose function-calling or thinking-mode semantics on the wire; the protocol stays frozen per ADR 0001.

## Architecture

Single `cargo workspace` at repo root. Planned crates (order of implementation):

| Crate | Role | Milestone |
|---|---|---|
| `inferd-proto` | Wire format: `Request`, `Response`, NDJSON read/write. `no_std`-friendly. | M1 |
| `inferd-daemon` | Binary — lifecycle, queue, single-instance lock, Unix socket / Windows named pipe / loopback TCP endpoints, activity log. | M1–M4 |
| `inferd-engine` | `Backend` trait + adapters. v0.1 ships `llamacpp` (FFI to vendored `libllama`) + `mock` (tests). v0.2 adds Anthropic / OpenAI / Bedrock / LiteLLM behind the same trait. | M2 |
| `inferd-stdio` | Same request handling as daemon, NDJSON over stdin/stdout, no listener. | later |

Clients (`clients/go/`, `clients/py/`, `clients/ts/`) are generated/hand-written wrappers shipped alongside the daemon. The Go client is the unblock for thlibo v0.2 deleting its embedded daemon.

### Flow at runtime

1. Daemon boots, acquires single-instance lock (flock on Unix, LockFileEx on Windows), initialises the configured backend(s).
2. Backend reports ready (for the v0.1 llama.cpp FFI backend: model load + KV-cache allocation succeed). **Only then** does the daemon create the socket / pipe / TCP listener — never before.
3. Client connects over NDJSON, sends `Request` frames.
4. Admission queue (default: 1 active generation, 10 queued). Overflow returns `{"type":"error","message":"queue full"}` immediately.
5. Router picks a backend per ADR 0007 policy (v0.1: trivial — one backend). Backend streams tokens back as `{"type":"token",...}` frames, terminates with one `done` or one `error`.
6. Client disconnect cancels the in-flight job. No retry/fallback in the daemon, no mid-stream failover — the caller owns retry.

## Non-negotiable invariants (from `context.md` §"Invariants")

When porting or extending, these are already-paid-for lessons — do not re-open:

1. Daemon has **zero knowledge** of middlewares, processors, or prompts. Messages array + sampling params in; tokens out.
2. Fallback on error is the **caller's** responsibility. Daemon reports cleanly; no retry/degrade/rewrite.
3. One active generation, bounded queue. `ErrFull` is non-blocking.
4. Single-instance lock file must **reject pre-existing symlinks** (thlibo threat-model #21).
5. Sockets invisible until backend `ready` fires.
6. No elevation. Per-user daemon. Unix socket `0660`, group `inferd-users`.
7. 64 MiB NDJSON frame cap, explicit byte limit (finding #5).
8. SHA-256 verification of downloaded models uses **constant-time** compare (finding #4, `subtle` crate).
9. Activity log NDJSON → `~/.inferd/logs/*.ndjson`, `INFERD_LOG=0|1|debug`, 3-generation rotation, secret redactor at write time (findings #8, #13).
10. Every `std::process::Command` is reviewed. v0.1 has **no subprocess engines** (per ADR 0005, llama.cpp is linked via FFI). Any future `Command` invocation is a code smell needing justification.

[ADR 0005](docs/adr/0005-libllama-ffi-not-subprocess.md) (supersedes 0003): the v0.1 default backend is `libllama` linked via FFI from a vendored `llama.cpp` submodule. No subprocess llamafile. No HTTP server compiled into the daemon. The `Backend` trait stays the same; only the default adapter changes.

[ADR 0006](docs/adr/0006-lean-core-ecosystem-extensions.md): lean-core posture. The daemon ships NDJSON-over-IPC + `Backend` trait + admission queue + router + security perimeter. **HTTP, OpenAI-compat, web UI, gRPC are not in the daemon** — they live as separate processes that talk NDJSON to inferd. Apps do not override the backend on the wire; if an app wants per-call control, it writes its own provider SDK integration.

[ADR 0007](docs/adr/0007-backend-routing-and-failure-semantics.md): routing is operator-configured policy across registered backends. **No in-daemon retry. No mid-stream failover.** Circuit breaker is the only stateful policy mechanism. v0.1 router is a no-op (one backend); v0.2 adds cloud adapters + real policy.

[ADR 0008](docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md) (supersedes 0001): protocol v1 is designed for inferd, not derived from thlibo. v1 frames carry `stop_reason` and `backend` on `done` frames, and `code` on `error` frames. thlibo will be refactored by its maintainer to consume the inferd Go client; the daemon does not bend its envelope to match an external client.

[ADR 0009](docs/adr/0009-pre-m1-open-questions-resolved.md): admin socket is a separate endpoint (`0600` on Unix), peer credentials enforced on UDS + named pipe (TCP gets API key only), protocol versioning is separate-socket-per-version (no in-band negotiation), backend identity exposed via `backend` field on `done` frames.

## Scope gates (what NOT to build)

- No HTTP/gRPC transport in the daemon — ever (ADR 0006). IPC only. HTTP is an ecosystem-extension job, separate process.
- No OpenAI-compat surface in the daemon — ever (ADR 0006). Same pattern.
- No per-request backend override on the wire — ever (ADR 0006, ADR 0007). Apps wanting per-call provider control should write their own SDK integration.
- No in-daemon retry on backend failure (ADR 0007). Caller owns retry.
- No mid-stream failover, ever (ADR 0007). Structurally broken; explicitly rejected.
- No subprocess engines in v0.1 (ADR 0005). llama.cpp is linked, not spawned.
- No multi-model warm pool in v0.1. One warm model at a time.
- No new v1 wire fields. Match thlibo v1 exactly. Extensions go to v2 on a separate socket.
- No async runtime pluralism. Tokio everywhere.
- No cloud backends in v0.1 (v0.2). The `Backend` trait + router are designed for them; adapters aren't built yet.

## Commands (once code lands)

Workspace is empty today; these commands will start working as crates are scaffolded in M1+.

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo audit
cargo deny check
```

Integration tests that need a real llamafile are gated behind the `llamafile-integration` cargo feature (per `CONTRIBUTING.md`). Run a single test with `cargo test -p <crate> <test_name>`.

The drop-in-replacement validation (M2 exit criteria): point thlibo's integration harness at `inferd-daemon --backend llamafile` and confirm the full suite passes.

## When writing ADRs

- Location: `docs/adr/NNNN-kebab-case-title.md`, sequential.
- ADRs are **immutable** once accepted. Supersede by writing a new one; set the old `Status:` to `superseded by NNNN`. Update `docs/adr/README.md` index.
- Required when: a decision crosses crate boundaries, changes the wire contract, changes security posture, picks a foundational dependency, or commits the team to a long-lived convention. Skip for local choices.
