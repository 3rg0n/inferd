# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

**Alpha.** v0.1 is in flight: `inferd-proto`, `inferd-engine`, `inferd-daemon`, and `inferd-client` are scaffolded and pass their lint+test cycles. Treat `docs/plan-v0.1.md`, `context.md`, `docs/protocol-v1.md`, and `docs/adr/` as the authoritative spec. Workspace members in `Cargo.toml` reflect what currently builds.

Before writing code, read `context.md` — it is the hand-off brief for new contributors and names the non-negotiable invariants the daemon must preserve.

## What inferd is

inferd is a single host-wide local-inference daemon. One warm model in memory, many consumers over NDJSON-over-IPC. It is plumbing, not a product — anything on the machine that wants local inference (CLI tools, IDE assistants, agent runtimes, web apps, middleware) connects to inferd instead of bundling its own engine.

Reference consumer projects exist as examples, but inferd does not encode any consumer's assumptions. The daemon's only contract is the wire protocol.

## Wire protocol is frozen

Protocol v1 is designed for inferd on its own merits ([ADR 0008](docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md), supersedes 0001). It is **immutable once shipped**. Do not change framing, rename fields, or break existing field semantics. Breaking changes become v2 on a **separate socket path** — no in-band version negotiation. See `docs/protocol-v1.md`.

Frame cap: 64 MiB per line. Use a bounded reader, not an auto-growing buffer.

Backwards-additive changes within v1 (new optional fields older servers MUST ignore and older clients MUST NOT require) are acceptable; v0.1 enforces "unknown fields ignored on parse" so the door for additive changes stays open.

## Model store

Models live in a shared content-addressable store at `$MODELS_HOME` per [ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md). Resolution order:

1. `models_home` field in `~/.inferd/config.json`.
2. `MODELS_HOME` env var.
3. Platform default: `${XDG_DATA_HOME:-$HOME/.local/share}/models/` (Linux), `~/Library/Application Support/models/` (macOS), `%LOCALAPPDATA%\models\` (Windows).

Layout: `blobs/sha256/<aa>/<full-hash>/data` for blobs, `manifests/<name>.json` for the name-to-blob map, `locks/<name>.lock` for advisory writer locks. Producers stream into `blobs/sha256/<aa>/.partial-<hash>/data.tmp`, verify SHA, atomic-rename, then write the manifest.

The store is wire-compatible with the cross-tool *Shared Local Model Store* convention so other tools that adopt the same shape can reuse blobs inferd has fetched and vice versa.

## Model reference material

`docs/` contains upstream Gemma 4 reference docs — not inferd design, but context on the model the default `llamacpp` backend serves:

- `run-gemma-content-generation-and-inferences.md` — framework/variant landscape
- `text.function.calling.with.gemma.4.md` — tool-use schema (potential v0.2+ protocol extension)
- `thinking.mode.in.gemma.md` — reasoning trace separation (potential v0.2+ response-frame extension)

Treat these as background, not requirements. v0.1 does not expose function-calling or thinking-mode semantics on the wire; the protocol stays frozen per ADR 0008.

## Architecture

Single `cargo workspace` at repo root. Crates:

| Crate | Role | Status |
|---|---|---|
| `inferd-proto` | Wire format: `Request`, `Response`, NDJSON read/write. `no_std`-friendly. | shipping |
| `inferd-daemon` | Binary — lifecycle, queue, single-instance lock, Unix socket / Windows named pipe / loopback TCP endpoints, admin socket, activity log, CAS model store, fetch. | shipping |
| `inferd-engine` | `Backend` trait + adapters. v0.1 ships `llamacpp` (FFI to vendored `libllama`) + `mock` (tests). v0.2 adds Anthropic / OpenAI / Bedrock / LiteLLM behind the same trait. | shipping (llamacpp + mock) |
| `inferd-client` | Rust client: NDJSON-over-IPC client + admin subscriber + connect-and-retry helpers. Published to crates.io. | shipping |
| `inferd-stdio` | Same request handling as daemon, NDJSON over stdin/stdout, no listener. | later |

Clients (`clients/go/`, future `clients/py/`, `clients/ts/`) are hand-written wrappers shipped alongside the daemon. The Go client is the canonical example for non-Rust consumers.

### Flow at runtime

1. Daemon boots, acquires single-instance lock (flock on Unix, LockFileEx on Windows). Admin socket is bound *immediately* so progress UIs can connect during the rest of bring-up.
2. Daemon reads `~/.inferd/config.json`, opens the model store, resolves or fetches the configured model into the CAS layout, verifies SHA-256 with constant-time compare.
3. Backend reports ready (for the v0.1 llama.cpp FFI backend: model load + KV-cache allocation succeed). **Only then** does the daemon create the inference socket / pipe / TCP listener — never before.
4. Client connects over NDJSON, sends `Request` frames.
5. Admission queue (default: 1 active generation, 10 queued). Overflow returns `{"type":"error","code":"queue_full",...}` immediately.
6. Router picks a backend per ADR 0007 policy (v0.1: trivial — one backend). Backend streams tokens back as `{"type":"token",...}` frames, terminates with one `done` or one `error`.
7. Client disconnect cancels the in-flight job. No retry/fallback in the daemon, no mid-stream failover — the caller owns retry.

## Non-negotiable invariants (from `context.md` §"Invariants")

When extending or refactoring, these are already-paid-for lessons — do not re-open:

1. Daemon has **zero knowledge** of middlewares, processors, or prompts. Messages array + sampling params in; tokens out.
2. Fallback on error is the **caller's** responsibility. Daemon reports cleanly; no retry/degrade/rewrite.
3. One active generation, bounded queue. `ErrFull` is non-blocking.
4. Single-instance lock file must **reject pre-existing symlinks** (THREAT_MODEL F-2).
5. Inference socket invisible until backend `ready` fires (THREAT_MODEL F-13). Admin socket is bound earlier so progress events are visible during bring-up.
6. No elevation. Per-user daemon. Unix socket `0660`, group `inferd-users`. Admin socket `0600`.
7. 64 MiB NDJSON frame cap, explicit byte limit (THREAT_MODEL F-5).
8. SHA-256 verification of downloaded models uses **constant-time** compare (`subtle` crate).
9. Activity log NDJSON → `~/.inferd/logs/*.ndjson`, `INFERD_LOG=0|1|debug`, 3-generation rotation, secret redactor at write time.
10. Every `std::process::Command` is reviewed. v0.1 has **no subprocess engines** (per ADR 0005, llama.cpp is linked via FFI). Any future `Command` invocation is a code smell needing justification.
11. The daemon may make outbound HTTPS only for the narrow purpose carved by [ADR 0010](docs/adr/0010-narrow-https-exception-for-model-bootstrap.md): one URL, one SHA, one file. No HTTP server, no OpenAI-compat, no registry browsing, no HTTP after `ready`.

[ADR 0005](docs/adr/0005-libllama-ffi-not-subprocess.md) (supersedes 0003): the v0.1 default backend is `libllama` linked via FFI from a vendored `llama.cpp` submodule. No subprocess llamafile. No HTTP server compiled into the daemon. The `Backend` trait stays the same; only the default adapter changes.

[ADR 0006](docs/adr/0006-lean-core-ecosystem-extensions.md): lean-core posture. The daemon ships NDJSON-over-IPC + `Backend` trait + admission queue + router + security perimeter. **HTTP, OpenAI-compat, web UI, gRPC are not in the daemon** — they live as separate processes that talk NDJSON to inferd. Apps do not override the backend on the wire; if an app wants per-call control, it writes its own provider SDK integration.

[ADR 0007](docs/adr/0007-backend-routing-and-failure-semantics.md): routing is operator-configured policy across registered backends. **No in-daemon retry. No mid-stream failover.** Circuit breaker is the only stateful policy mechanism. v0.1 router is a no-op (one backend); v0.2 adds cloud adapters + real policy.

[ADR 0009](docs/adr/0009-pre-m1-open-questions-resolved.md): admin socket is a separate endpoint (`0600` on Unix), peer credentials enforced on UDS + named pipe (TCP gets API key only), protocol versioning is separate-socket-per-version (no in-band negotiation), backend identity exposed via `backend` field on `done` frames.

[ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md): models live in a shared CAS store at `$MODELS_HOME`. Manifest indirection (`name → sha`) plus content-addressed blob paths. Wire-compatible with the cross-tool convention.

## Scope gates (what NOT to build)

- No HTTP/gRPC transport in the daemon — ever (ADR 0006). IPC only. HTTP is an ecosystem-extension job, separate process. The narrow ADR 0010 HTTPS exception is for model bootstrap only and explicitly forbids serving HTTP.
- No OpenAI-compat surface in the daemon — ever (ADR 0006). Same pattern.
- No per-request backend override on the wire — ever (ADR 0006, ADR 0007). Apps wanting per-call provider control should write their own SDK integration.
- No in-daemon retry on backend failure (ADR 0007). Caller owns retry.
- No mid-stream failover, ever (ADR 0007). Structurally broken; explicitly rejected.
- No subprocess engines in v0.1 (ADR 0005). llama.cpp is linked, not spawned.
- No multi-model warm pool in v0.1. One warm model at a time.
- No registry browsing, model search, or arbitrary HTTP fetches in the daemon (ADR 0010). The fetch surface is one URL + one SHA.
- No new v1 wire fields. v1 is frozen (ADR 0008). Extensions go to v2 on a separate socket.
- No async runtime pluralism. Tokio everywhere.
- No cloud backends in v0.1 (v0.2). The `Backend` trait + router are designed for them; adapters aren't built yet.

## Commands

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo audit
cargo deny check
```

Integration tests that need a real llama.cpp build are gated behind the `llamacpp-integration` cargo feature. Run a single test with `cargo test -p <crate> <test_name>`.

## When writing ADRs

- Location: `docs/adr/NNNN-kebab-case-title.md`, sequential.
- ADRs are **immutable** once accepted. Supersede by writing a new one; set the old `Status:` to `superseded by NNNN`. Update `docs/adr/README.md` index.
- Required when: a decision crosses crate boundaries, changes the wire contract, changes security posture, picks a foundational dependency, or commits the team to a long-lived convention. Skip for local choices.
