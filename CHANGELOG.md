# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Repo scaffolding only: README, LICENSE (MIT), CONTRIBUTING, context
  hand-off brief, v0.1 plan, protocol-v1 spec, four ADRs.
- `.gitignore` with Rust + secrets + model-artefact patterns.
- `docs/ai.internals.explained.md` — 15-component explainer of how
  local LLM serving stacks are built; standalone reference.
- `CLAUDE.md` at repo root — guidance for future Claude Code sessions
  working in this repository.
- ADR 0005 — consume `libllama` via FFI rather than running llamafile
  as a subprocess. Supersedes 0003.
- ADR 0006 — lean-core posture. HTTP, OpenAI-compat, web UI, and
  per-app backend override are out of the daemon and live as
  ecosystem extensions in separate processes.
- ADR 0007 — backend routing model. Operator-configured policy,
  no in-daemon retry, no mid-stream failover, circuit breaker as the
  only stateful policy mechanism.
- ADR 0008 — protocol v1 designed for inferd on its own merits;
  supersedes 0001. v1 adds `stop_reason` and `backend` on `done`
  frames and `code` on `error` frames. thlibo is refactored to
  match, not the other way around.
- ADR 0009 — pre-M1 open questions resolved (admin socket as a
  separate `0600` endpoint, peer credentials enforced on UDS +
  named pipe, protocol versioning via separate sockets, backend
  identity in `done` frames).
- `docs/protocol-v1.md` rewritten to reflect ADR 0008 — clean
  inferd-native spec with `stop_reason`, `backend`, and
  `code` fields documented.
- `THREAT_MODEL.md` skeleton at repo root with 16 findings
  (F-1 through F-16). v0.1 GA blocks until every "applies"
  finding is `mitigated` with a code reference.
- `vendor/llama.cpp/PIN.md` recording the chosen llama.cpp
  pin (tag `b9159`, 2026-05-15) and the bump procedure. The
  submodule itself is added in M2a.
- `docs/test-strategy.md` describing the six test tiers
  (unit, daemon-integration with mock, engine-integration
  with libllama, functional replacement, security
  regression, fuzzing), platform matrix, and cargo features.
- `Cargo.toml` skeletons for all four crates
  (`inferd-proto`, `inferd-engine`, `inferd-daemon`,
  `inferd-stdio`) — package metadata, dependency lists,
  feature flags. No `lib.rs`/`main.rs` yet; workspace
  `members` array stays commented out until M1 adds the
  source files.

### Changed

- `docs/plan-v0.1.md` M2 retitled "llama.cpp FFI backend" with
  updated implementation steps and exit criteria. Routing section
  added (no-op in v0.1, real in v0.2).
- ADR 0003 status flipped to `superseded by 0005`. Body unchanged
  per ADR-immutability rule.
- ADR 0001 status flipped to `superseded by 0008`. Body unchanged
  per ADR-immutability rule.
- `docs/plan-v0.1.md` "Threat model" section replaced with a
  pointer to `THREAT_MODEL.md` and a per-milestone mitigation
  schedule. "Open questions for the implementer" section replaced
  with a pointer to ADR 0009.
- `CLAUDE.md` invariant #10 and architecture summary updated to
  reflect ADRs 0005, 0006, 0007. Scope-gates section expanded with
  the explicit "no HTTP, ever" / "no per-request backend override,
  ever" rules.

**M1 in progress**: `inferd-proto`, `inferd-engine`, and
`inferd-daemon` (lock + queue) landed.

- `inferd-proto`: types, NDJSON framing with 64 MiB bounded
  reader (THREAT_MODEL F-1), request validation. 15 tests.
- `inferd-engine`: `Backend` async trait, `TokenEvent`/
  `TokenStream`, `GenerateError`, deterministic `mock`
  adapter with failure-mode injection (pre-stream error,
  mid-stream drop, ready toggle). 7 tests.
- `inferd-daemon` section A: cross-platform single-instance
  `Lock` (uses `std::fs::File::try_lock`, stable since 1.89),
  symlink rejection (THREAT_MODEL F-2), bounded admission
  `Queue` (1 active + N queued, non-blocking submit, returns
  `SubmitError::QueueFull`). 8 tests.
- `inferd-daemon` section B: endpoint listeners. `bind_tcp`
  for cross-platform loopback TCP (default `127.0.0.1:47321`),
  `bind_uds` (Unix only) for Unix domain sockets with mode
  `0660`, optional group ownership via `nix::unistd::chown`,
  and pre-binding symlink refusal. Windows named pipe
  deferred to M4. `Connection` trait abstracts UDS/TCP
  uniformly. 4 tests (3 enabled per platform).
- `inferd-daemon` section D: M1 exit-criterion integration test
  (`tests/echo.rs`). Boots the lifecycle in-process against the
  mock backend over loopback TCP, connects a real client, and
  asserts the full request → token → done flow. Coverage:
  golden path with id echo + content concat + stop_reason +
  backend per ADR 0008; invalid_request error frame; mid-stream
  drop produces `code: backend_unavailable` per ADR 0007;
  ready-gating regression for THREAT_MODEL F-13. 4 tests.

### M2b — `LlamaCpp` Backend adapter

- `inferd-engine::llamacpp` module: real `Backend` impl built on
  the FFI bindings. Compiled in only when the `llamacpp` feature
  is enabled; default builds remain mock-only.
- `loader.rs`: `load_model()` opens the GGUF file, optionally
  verifies SHA-256 against an expected hash using
  `subtle::ConstantTimeEq` (THREAT_MODEL F-5), then hands off to
  `llama_model_load_from_file`. `ModelHandle` owns the
  `llama_model*` and runs `llama_model_free` on drop. F-6 TOCTOU
  caveat documented inline.
- `backend.rs`: `LlamaCpp::new()` initialises libllama (idempotent
  via `Once`), loads the model, allocates `llama_context` with
  configurable `n_ctx`, flips ready. `generate()` renders the
  model's chat template, tokenizes, then spawns a `spawn_blocking`
  task that drives `llama_decode` + `llama_sampler_sample` and
  streams `TokenEvent`s through a tokio mpsc channel.
- Sampler chain: `top_k → top_p → temp → dist`, with grammar
  inserted first when `Resolved::grammar` is non-empty (GBNF via
  `llama_sampler_init_grammar`). Wires THREAT_MODEL F-11 — no
  parse-time complexity bound yet, deferred per the threat-model
  doc.
- Cancellation: dropping the response stream drops the receiver,
  which causes `tx.blocking_send` in the C++ loop to error and the
  loop exits. KV cache is reset between generations via
  `llama_memory_clear`.
- Build script: switched to `Release` CMake configuration so the
  C++ CRT matches Rust's release-CRT linkage on Windows. Linked
  `Advapi32.lib` for `ggml-cpu`'s registry probes.
- `tests/llamacpp.rs`: 3 tier-3 tests behind
  `--features llamacpp-integration`. Skip cleanly with
  `[skip] INFERD_TEST_MODEL_PATH not set` when no model is
  available; otherwise verify load → stream → done with
  `stop_reason ∈ {End, Length}` and `completion_tokens > 0`,
  cancellation behaviour, and `InvalidRequest` for empty messages.

48/48 tests pass under default features
(15 proto + 9 engine + 20 daemon + 4 daemon-integration).
12/12 with `llamacpp-integration` feature on (engine adapter
compiles and tier-3 stubs skip clean). Workspace clippy clean
in both configurations. `cargo audit` reports zero advisories
across 137 dependencies.

### M2a — llama.cpp build wiring

- `vendor/llama.cpp` submodule pinned at tag `b9159` (commit
  `5c0e94683`, dated 2026-05-15). Activated with
  `git submodule update --init --recursive`.
- `inferd-engine/build.rs`: behind feature `llamacpp`, runs
  CMake on the submodule with `LLAMA_BUILD_SERVER`/`EXAMPLES`/
  `TESTS`/`TOOLS=OFF`, `LLAMA_CURL=OFF`, `BUILD_SHARED_LIBS=OFF`.
  Generates Rust bindings via bindgen 0.71 from
  `vendor/llama.cpp/include/llama.h` into
  `OUT_DIR/llama_bindings.rs`. GPU backends (`cuda`, `metal`,
  `vulkan`, `rocm`) opt-in via cargo features; default is CPU-only.
- `inferd-engine::ffi` includes the generated bindings. Crate
  lint posture changed from `forbid(unsafe_code)` to
  `deny(unsafe_code)` so the FFI module can scope an inner
  `allow` to bindgen output. Every other module remains
  unsafe-free.
- Default `cargo build` (no features) still works without a
  C++ toolchain or `libclang` — the build script short-circuits
  on `CARGO_FEATURE_LLAMACPP`.
- Smoke test on Windows 11: `cargo build -p inferd-engine
  --features llamacpp` produces `llama.lib`, `ggml*.lib` static
  archives plus a 1,865-line `llama_bindings.rs`. Workspace
  clippy and tests pass with the feature on and off.

### M1 status — ✅ exit criteria met

46/46 tests pass workspace-wide on Windows + the test suite
proves the protocol invariants from ADR 0008, the routing
semantics from ADR 0007, and the F-13 ready-gating mitigation.
Crate ships an `inferd-daemon` binary you can run locally
against `--backend mock`. Engine adapter for llama.cpp is M2.

- `inferd-daemon` section C: router, lifecycle, config, main.
  `Router` (no-op v0.1 per ADR 0007) picks a single backend.
  `lifecycle::handle_connection` reads `Request` frames, routes
  through the `Backend`, and writes `Response::Token`/`Done`
  with `stop_reason` and `backend` per ADR 0008. Mid-stream
  failures emit `error` with `code: backend_unavailable`.
  `lifecycle::wait_for_ready` polls every 50ms up to a
  configured timeout (THREAT_MODEL F-13 — listener bound
  AFTER ready). `lifecycle::serve_tcp` and `serve_uds`
  accept-loops with shutdown via tokio oneshot channel
  (SIGTERM/SIGINT on Unix, Ctrl-C on Windows). `clap`-based
  CLI in `config.rs` with `--lock`, `--tcp`, `--uds`,
  `--group`, `--queue-depth`, `--ready-timeout-secs`. Crate
  now ships an `inferd-daemon` binary. 8 new tests
  (router 3, config 4, lifecycle 3) — 20 daemon tests total.

### Changed
- Workspace MSRV bumped 1.76 → 1.89 to use `File::try_lock`
  from `std`. The previous floor was speculative; 1.89 is
  current.

30/30 tests pass on Rust 1.92. Clippy clean. Daemon
sections B–D still pending. See `docs/plan-v0.1.md`.
