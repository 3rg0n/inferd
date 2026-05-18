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

## Wire protocol

Authoritative spec: `docs/protocol-v1.md` (designed for inferd
per ADR 0008; v1 immutable once shipped). Frame cap: 64 MiB per
line. Request fields and the `Response` enum live in
`crates/inferd-proto/`.

## Milestones

Status legend: ✅ shipped, code on `main`. ⏳ partial, see
section. 🅿️ deferred (out of v0.1 scope).

### M0 — scaffolding ✅

Workspace + crate dirs + `docs/` + ADRs 0001–0009 + threat-model
skeleton + vendor pin doc. Commit `19e4687` and `50aa26d`.

### M1 — proto + echo daemon ✅

- `inferd-proto`: `Request`/`Resolved`, `Response` enum with
  ADR 0008 fields (`stop_reason`, `backend`, `code`), bounded
  64 MiB NDJSON reader (F-1).
- `inferd-engine`: `Backend` trait, `TokenEvent`/`TokenStream`,
  `mock` adapter with failure-mode injection.
- `inferd-daemon`: lock with symlink reject (F-2), bounded
  admission queue, UDS + loopback TCP listeners, no-op
  `Router`, `lifecycle::handle_connection`, clap-driven CLI,
  ready-gated listener creation (F-13).
- M1 exit criterion (`tests/echo.rs`): real client connects,
  sends `Request`, receives streamed tokens + `Done` carrying
  `backend=mock` and `stop_reason=end`.

### M2a — `libllama` build wiring ✅

- `vendor/llama.cpp` submodule at tag `b9159` (commit `5c0e94683`).
- `inferd-engine/build.rs` runs CMake (release CRT on Windows,
  `LLAMA_BUILD_SERVER/EXAMPLES/TESTS/TOOLS=OFF`,
  `LLAMA_CURL=OFF`, `BUILD_SHARED_LIBS=OFF`); generates Rust
  bindings via `bindgen` 0.71 against `include/llama.h`.
- GPU backends as opt-in cargo features (`cuda`, `metal`,
  `vulkan`, `rocm`); CPU-only by default.

### M2b — `LlamaCpp` Backend adapter ✅

- `loader.rs`: `load_model()` with optional SHA-256 verification
  using `subtle::ConstantTimeEq` (F-5). `ModelHandle` /
  `ContextHandle` RAII.
- `backend.rs`: chat-template render → tokenize → spawn-blocking
  decode/sample loop → tokio mpsc → cancellation-by-drop. Sampler
  chain: `grammar → top-k → top-p → temp → dist`, with GBNF
  enforced when `Resolved::grammar` is non-empty.

### M2c — daemon + real-model tier-3 test ✅ (test harness; runtime
verification deferred to operator)

- `BackendKind::Llamacpp` variant (gated behind `llamacpp` feature).
- CLI flags: `--model-path`, `--model-sha256`, `--n-ctx`,
  `--n-gpu-layers`.
- `tests/echo_llamacpp.rs`: skips with explanatory message when
  `INFERD_TEST_MODEL_PATH` is unset; otherwise boots the
  lifecycle with a real `LlamaCpp` adapter, drives a short
  request, asserts `backend=llamacpp` + non-zero
  `completion_tokens`.
- **Open**: nobody has actually run this against a Gemma 4 GGUF
  yet. Adapter is type-correct and FFI-aligned; the model load
  + decode round-trip is unverified by humans. Smoke this before
  GA.

### M3 — activity log + redactor ✅

- `logx::LogxWriter` with rolling rotation (`.ndjson` → `.1` →
  `.2` → `.3`, keep 3 generations — F-4).
- `logx::LogxLayer` `tracing_subscriber::Layer` serialises
  events as NDJSON: `t`, `level`, `component`, `msg`, +
  structured fields.
- `redact::redact_in_place` runs *inside*
  `LogxWriter::write_record` so debug-level dumps are scrubbed
  before disk write (F-3).
- `lifecycle::handle_connection` emits `request_done` /
  `request_error_mid_stream` events.
- `tests/logx.rs`: integration test verifies the
  `request_done` record shape *and* that an injected
  credential string does not appear in the on-disk log.

### M4 — cross-platform IPC + packaging ⏳

#### M4a — Windows named pipe ✅

- `endpoint::bind_named_pipe(path, first)` wraps tokio's
  `ServerOptions`. `lifecycle::serve_named_pipe` implements the
  multi-instance accept pattern with caller-supplied first
  instance (no listener-before-spawn race).
- `--pipe` CLI flag mutually exclusive with `--tcp`/`--uds`.
- `tests/echo_pipe.rs`: 2 tests including
  `multi_instance_accept_serves_two_sequential_clients`.

#### M4b — release workflow ✅

- `.github/workflows/ci.yml`: fmt + clippy + test on
  `[ubuntu, macos, windows]`, with and without the `llamacpp`
  feature. Go-client job builds the daemon binary then runs
  `go vet` + `go test`. `cargo audit` on push-to-main +
  schedule (does not block PRs).
- `.github/workflows/release.yml`: tag-triggered (`v*`) matrix
  build (linux x86_64, linux aarch64 via `cross`, macos
  aarch64, windows x86_64). Generates CycloneDX SBOM via
  `cargo cyclonedx`, signs each archive with keyless cosign,
  publishes to GitHub Release.
- F-15 (signed releases + SBOM) → mitigated.

#### M4c — service-manifest install 🅿️ → packaging follow-up

systemd unit / launchd plist / Windows service registration
land alongside packaging in `packaging/`. Scope-cut from the
alpha commit: see [F-16 systemd hardening](#post-alpha-tracked-work).

### M5 — Go client ✅

- `clients/go/` Go module at
  `github.com/3rg0n/inferd/clients/go`. `Client` with `DialTCP`,
  `DialUDS` (Unix), `DialPipe` (Windows). `Generate(ctx, req)`
  returns a frame channel; `ctx` cancel closes the connection.
- `client_test.go`: protocol-shape round-trip + end-to-end
  against the Rust daemon binary (auto-locates
  `target/debug/inferd-daemon[.exe]`).
- Exit criterion is "thlibo v0.2 imports this module and tests
  pass." That's a downstream concern — verify when thlibo's
  v0.2 branch picks this up.

### M6 (v0.2) — cloud backend adapters 🅿️

Out of scope for v0.1. ADR 0007 sketches the routing model so
the `Backend` trait + `Router` shape can absorb Ollama / OpenAI
/ Bedrock / Anthropic adapters when v0.2 starts.

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

## Post-alpha tracked work

The `0.1.0-alpha.1` tag ships everything above; these items are
real but not GA-blocking on their own. Each has a `THREAT_MODEL.md`
finding pointer.

| Item | Finding | Notes |
|---|---|---|
| Real Gemma 4 GGUF run | M2c above | Operator drives once a model file is in hand |
| FFI crash isolation (sandboxed worker) | F-9 | Accepted risk for v0.1; v0.3+ if recurring crashes show |
| `inferd-stdio` crate | plan §"crate layout" | Stub Cargo.toml only; sources land when a caller needs it |
| Tier 5 `security` feature aggregating regression tests | `docs/test-strategy.md` | Tests exist scattered; the feature flag does not |
| Tier 6 fuzzing | `docs/test-strategy.md` | `cargo +nightly fuzz` against the proto frame parser |
| Python + TypeScript clients | `clients/{py,ts}/` | Stubs only; out of v0.1 scope |

**Closed in alpha.2** (2026-05-16): F-7 peer credentials,
F-8 TCP API-key auth, F-16 Linux + macOS hardening manifests
(see CHANGELOG.md `[0.1.0-alpha.2]`).

**Closed in pre-GA work**:
- F-6 TOCTOU mitigation: copy-to-tempdir before hash + load in
  `inferd-engine::llamacpp::loader::load_model`.
- F-11 GBNF parse-time complexity bound:
  `inferd-engine::llamacpp::backend::validate_grammar` (length +
  alternation caps before forwarding to libllama).
- F-16 Windows service-ACL via `sc.exe sdset` in
  `packaging/windows/install.ps1`.
