# inferd v0.1 plan

- **Status**: draft
- **Date**: 2026-05-14
- **Scope**: v0.1 — drop-in replacement for thlibo's embedded
  `thlibod` daemon. No cloud backends, no model-proxy-gateway mode
  yet (that's v0.2).
- **Posture**: lean core (ADR 0006). The daemon ships an inference
  engine consumed via FFI (ADR 0005), an admission queue, NDJSON
  IPC transport, and a routing layer (ADR 0007 — no-op in v0.1).
  HTTP, OpenAI-compat, web UI, and per-app backend override are
  all out of scope and live as ecosystem extensions in separate
  processes.

## Goal

Stand up a standalone Rust inference daemon that thlibo v0.2 can
consume as a dependency, so deleting `thlibo/internal/daemon/` and
`thlibo/internal/ipc/` leaves a working product.

## Non-goals for v0.1

- Cloud backends (OpenAI, Bedrock, Anthropic, LiteLLM). Architected
  for — the `Backend` trait is designed in from day one — but not
  implemented.
- HTTP/gRPC transport. IPC-only (Unix socket / Windows named pipe /
  loopback TCP).
- Multi-model warm pool. One warm model at a time in v0.1.
- KV cache sharing across connections.
- Attestation / signing of release artefacts (defer to v0.2, matches
  thlibo's own deferral for #27/#28).

## Crate layout

```
inferd/
├── Cargo.toml                  # workspace manifest
├── crates/
│   ├── inferd-proto/           # wire format, no_std-friendly
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── request.rs      # Request, Message, Role
│   │   │   ├── response.rs     # Response enum: Token, Done, Error, Status
│   │   │   └── frame.rs        # NDJSON read/write with 64 MiB cap
│   │   └── Cargo.toml
│   │
│   ├── inferd-engine/          # Backend trait + adapters
│   │   ├── src/
│   │   │   ├── lib.rs          # trait Backend
│   │   │   ├── llamafile.rs    # spawn + stdio protocol (v0.1 target)
│   │   │   └── mock.rs         # test double, used by daemon integration tests
│   │   └── Cargo.toml
│   │
│   ├── inferd-daemon/          # the binary
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── lifecycle.rs    # boot, accept, dispatch, shutdown
│   │   │   ├── queue.rs        # fixed-depth admission queue
│   │   │   ├── lock.rs         # single-instance lock (flock / LockFileEx)
│   │   │   ├── endpoint_unix.rs
│   │   │   ├── endpoint_windows.rs
│   │   │   ├── endpoint_tcp.rs
│   │   │   ├── logx.rs         # NDJSON activity log + redactor
│   │   │   └── config.rs       # env + CLI flags
│   │   └── Cargo.toml
│   │
│   └── inferd-stdio/           # variant that speaks NDJSON on stdin/stdout
│       ├── src/main.rs         # same request handling, no listener
│       └── Cargo.toml
│
├── clients/
│   ├── go/                     # github.com/3rg0n/inferd-go
│   │   └── (generated from inferd-proto via a later milestone)
│   ├── py/
│   └── ts/
│
├── docs/
│   ├── plan-v0.1.md            # this file
│   ├── protocol-v1.md          # authoritative wire format
│   └── adr/
│       ├── README.md
│       ├── 0001-wire-protocol-inherited-from-thlibo.md
│       ├── 0002-rust-not-go.md
│       ├── 0003-subprocess-llamafile-not-ffi.md
│       └── 0004-apache-not-considered-license-mit.md
│
├── context.md
├── LICENSE                     # MIT
├── .gitignore
└── README.md
```

## Dependencies (tentative)

- **tokio** — single async runtime choice.
- **serde + serde_json** — NDJSON de/serialisation.
- **tracing + tracing-subscriber** — structured logging; export to
  NDJSON via a custom layer so `logx` field names match thlibo's.
- **anyhow + thiserror** — error plumbing.
- **clap** — CLI flag parsing.
- **nix** (Unix only) — `flock`, `SO_PEERCRED`, socket modes.
- **windows-sys** (Windows only) — named pipe SDDL, `LockFileEx`.
- **sha2** — GGUF verification.
- **subtle** — constant-time hash compare (carried over from thlibo
  threat-model finding #4).
- **bindgen** + **cmake** (build-time) — generate Rust bindings
  for `libllama` and build it from the vendored `llama.cpp`
  submodule. CI gets a C++17 toolchain on every target platform.

C FFI to `libllama` is the v0.1 default backend (ADR 0005). No
subprocess engines.

## Wire protocol (inherited)

See `docs/protocol-v1.md` (to be written from thlibo's
`internal/ipc/protocol.go`). Frame cap: 64 MiB per line. Request
fields are documented in `crates/inferd-proto/src/request.rs`.
Response stream uses a tagged enum with discriminator `type`.

## Milestones

### M0 — scaffolding ✅ (this commit)

- Workspace `Cargo.toml` (stub only for now; no Cargo.lock yet).
- Crate directories declared with README stubs so the layout is
  clear before any Rust is written.
- `docs/protocol-v1.md` placeholder pointing at thlibo's code.
- ADR 0001, 0002, 0003 stubs.

**Exit criteria**: `cargo check` fails cleanly with
"no targets in workspace" — because there aren't any yet. That's
fine; this milestone is pure planning surface.

### M1 — proto + echo daemon

- Implement `inferd-proto` end-to-end: `Request`, `Message`,
  `Response`, NDJSON frame read/write with size cap.
- Implement `inferd-daemon/endpoint_*` for Unix + TCP (Windows
  pipe can wait until M4). Just accept a connection, read one
  Request, respond with a single `Response::Done` frame echoing the
  request id.
- Port thlibo's `internal/daemon/lock.go` invariants (regular-file
  check, symlink reject).

**Exit criteria**: from the thlibo integration-test harness,
replace `thlibod` with `inferd-daemon --backend mock` and confirm
thlibo's daemon-level tests that don't need real inference pass.

### M2 — llama.cpp FFI backend

- Vendor `ggerganov/llama.cpp` as a git submodule under
  `vendor/llama.cpp/` at a pinned commit.
- `crates/inferd-engine/build.rs` runs CMake on the submodule with
  `LLAMA_BUILD_SERVER=OFF`, `LLAMA_BUILD_EXAMPLES=OFF`,
  `LLAMA_BUILD_TESTS=OFF`. GPU backends (CUDA, Metal, Vulkan,
  ROCm) are opt-in cargo features, off by default.
- Generate Rust bindings for `llama.h` via `bindgen`.
- Implement `inferd-engine::llamacpp::LlamaCpp` against the
  bindings: load model, allocate KV cache, run forward pass,
  stream tokens back via callback into the Rust async runtime.
- `Backend::ready()` flips true after model load + KV-cache
  allocation succeed.
- Wire the queue: 1 active, 10 queued, `ErrFull` on overflow, ctx
  cancellation propagates (drop the request → drop the in-flight
  generation handle).
- Streaming `Response::Token` frames back per generated token,
  no subprocess pipe in the loop.

**Exit criteria**: thlibo's full integration test suite (incl. the
~60s real-engine tests) passes against a running inferd. This is
the drop-in-replacement validation. *Also*: generated binary has
no llamafile or llama.cpp HTTP server symbols (verify with `nm` /
`dumpbin`); subprocess count during a generation is exactly 1
(the daemon itself).

### M3 — activity log + redactor

- Port `logx.go` → `crates/inferd-daemon/src/logx.rs`. Same record
  shape (`t`, `component`, `level`, `msg` + fields). Same env var
  (rename `THLIBO_LOG` → `INFERD_LOG`). Same rolling rotation (3
  generations). Same secret-pattern redactor.
- Default log dir: `~/.inferd/logs/`.

**Exit criteria**: integration test that sets `INFERD_LOG=debug`,
triggers one generation, and asserts the NDJSON record for
`request_done` has the expected fields.

### M4 — Windows named pipe + packaging

- `endpoint_windows.rs` using `windows-sys`; SDDL scoped to the
  current SID.
- `clap`-driven CLI mirroring thlibod's flags (`-engine`, `-lock`,
  `-infer-addr`, `-admin-addr`, `-group`, `-tcp`, `-ready-timeout`,
  `-stop-timeout`, `-v`).
- Cross-platform release workflow (matrix: linux/amd64,
  linux/arm64, darwin/arm64, windows/amd64) producing signed
  tarballs. Signing plan inherited from thlibo's #27/#28 v0.2 work.

**Exit criteria**: `inferd install` on each platform yields an
autostarted daemon that thlibo v0.2 can talk to on boot.

### M5 — Go client crate

- `clients/go/` is a Go module with a `Client` struct wrapping the
  NDJSON protocol. Module path:
  `github.com/3rg0n/inferd/clients/go` (sub-module in the monorepo).
- thlibo v0.2 imports this module and deletes its own daemon code.

**Exit criteria**: thlibo v0.2 branch compiles and tests pass with
`internal/daemon/` and `internal/ipc/` deleted.

### M6 (v0.2) — cloud backend adapters

Out of scope for v0.1. Sketched in an ADR so the `Backend` trait
design can be validated against the shape of Ollama / OpenAI /
Bedrock / Anthropic / LiteLLM requests before M1 freezes.

## Threat model

Findings live in `THREAT_MODEL.md` at the repo root. v0.1 GA
blocks until every "applies" finding is `mitigated` (with a
named code site) or has an explicit waiver in that document.
Each milestone is responsible for landing the mitigations
called out in its scope:

- M1: F-1 (frame cap), F-2 (lock symlink), F-13 (ready
  gating), F-14 (no subprocess regression).
- M2: F-5, F-6 (model verify), F-9 (FFI crash isolation),
  F-11 (GBNF resource bounds).
- M3: F-3 (log redactor), F-4 (log rotation).
- M4: F-7, F-8 (peer credentials, TCP caveat),
  F-15 (signed releases, SBOM), F-16 (daemon hardening).

## Routing (no-op in v0.1, real in v0.2)

ADR 0007 specifies the routing model: operator-configured policy
across registered backends, no in-daemon retry, no mid-stream
failover, circuit breaker as the only stateful policy mechanism.
v0.1 ships with a single backend (local llama.cpp) so the router
is structurally a no-op — it picks the only backend it has. The
shape (`Router`, policy choose-fn, breaker map) is wired in M2 so
that v0.2 can plug in cloud adapters without reshaping the
daemon.

The wire protocol exposes no per-request backend field. Apps do
not pick the backend. If an app wants direct, app-specific
control over a provider, it integrates that provider's SDK
directly into itself; that workload is not what inferd is for
(see ADR 0006).

## Pre-M1 open questions

All four pre-M1 open questions (admin socket, peer-credential
enforcement, protocol versioning, backend identity in `done`
frames) are settled in [ADR 0009](adr/0009-pre-m1-open-questions-resolved.md).
M1 starts against the resolved decisions; nothing in this
section is unresolved.
