# inferd test strategy

This document describes how inferd is tested. It is not a CI
config; it is the contract that the CI config implements.

## Goals

- Catch protocol regressions immediately. Wire bytes are the
  load-bearing public surface (ADR 0008); a test failure here
  is non-negotiable.
- Catch security regressions. Every `applies` finding in
  `THREAT_MODEL.md` has at least one test that fails if the
  mitigation is removed.
- Keep the developer-loop fast. Most tests run without a real
  GGUF model on disk and without a C++ toolchain.
- Make the "drop-in functional replacement" claim verifiable.
  M2 exit criterion is end-to-end test parity with thlibo's
  integration harness, pointed at a running inferd.

## Test tiers

### Tier 1 — unit tests

- `cargo test --all` (no features). Default for every PR.
- Must pass on every supported target without a model file or
  network access.
- Exercises: protocol parse/serialise round-trips, frame size
  cap behaviour, request validation, queue admission logic,
  lock-file symlink rejection, log redactor patterns,
  router policy choose-fn (with mock backends).

### Tier 2 — daemon integration with `mock` backend

- `cargo test --all` with the daemon binary built. Runs against
  a tokio-driven test harness that opens an actual UDS / named
  pipe, sends real NDJSON, and asserts response framing.
- Required to pass per PR.
- Exercises: full lifecycle (lock → ready → accept → dispatch
  → shutdown), peer-credential extraction (where feasible to
  test in a unit context), admin-socket status broadcast,
  cancellation on disconnect, queue-full behaviour, ready
  gating (asserts socket does not exist before backend ready).

### Tier 3 — engine integration with real `libllama`

- `cargo test --all --features llamacpp-integration`.
- Requires: C++ toolchain, the vendored `llama.cpp` submodule
  built, a Gemma 4 GGUF on disk at a path read from
  `INFERD_TEST_MODEL_PATH`.
- Skipped by default. Run nightly in CI on every supported
  platform; run on-demand by developers with a local model.
- Exercises: real inference round-trip, GBNF grammar
  enforcement (assert constrained output structure),
  cancellation propagation through the C++ generation loop,
  multi-request serialisation through the queue.

### Tier 4 — functional replacement validation

- The thlibo integration harness, repointed at a running
  inferd. (thlibo will be refactored against the inferd Go
  client; until then, the harness exercises raw NDJSON.)
- Required to pass before tagging v0.1.0.
- Exercises: any thlibo behaviour that crosses the wire —
  request shape, response shape, streaming, cancellation,
  errors. This is the load-bearing test for the "functional
  replacement" claim.

### Tier 5 — security regression tests

- A dedicated `security` feature: `cargo test --features
  security`.
- Must pass on every PR.
- Each test corresponds to one finding in `THREAT_MODEL.md`:
  - `f1_frame_cap` — write a 65 MiB line; assert
    `frame_too_large` error and connection close.
  - `f2_lock_symlink` — pre-create a symlink at the lock
    path; assert daemon refuses to start.
  - `f3_log_redactor` — enable debug logging, send a request
    containing each known secret pattern; assert log lines
    are redacted.
  - `f5_constant_time_compare` — synthetic test asserting the
    hash compare path uses `subtle::ConstantTimeEq` (lint or
    code-search test, not a timing measurement).
  - `f9_engine_input_validation` — send malformed-but-syntactic
    payloads to the engine adapter; assert the proto crate
    rejects them before the engine sees them.
  - `f13_ready_gating` — start daemon with a backend whose
    ready takes 2 s; assert connect() during that window
    fails with the platform's "no listener" error, not a
    successful connect.
  - `f14_no_subprocess` — assert `Command::new` does not
    appear in `inferd-engine` or `inferd-daemon` source.
- New findings require new tests in this tier before the
  finding moves to `mitigated`.

### Tier 6 — fuzzing

- `cargo +nightly fuzz` against the proto crate's frame
  parser, with a corpus seeded from real captured frames.
- Runs on a schedule, not per PR. Findings open issues, not
  block PRs.
- Coverage targets: NDJSON reader, request validator, GBNF
  pass-through (does not fuzz GBNF parsing — that lives
  inside llama.cpp).

## Cargo features

| Feature | Effect |
|---|---|
| `default` | Tier 1 + Tier 2. CPU-only. No `libllama`. |
| `llamacpp-integration` | Adds Tier 3 tests. Requires submodule + GGUF. |
| `security` | Adds Tier 5 tests. |
| `cuda`, `metal`, `vulkan`, `rocm` | GPU backends for the FFI engine. Off by default. |

## Platform matrix

CI runs the following matrix per PR:

| Platform | Tier 1 | Tier 2 | Tier 3 (nightly) | Tier 5 |
|---|---|---|---|---|
| Linux x86_64 | ✓ | ✓ | ✓ | ✓ |
| Linux ARM64 | ✓ | ✓ | ✓ | ✓ |
| macOS ARM64 | ✓ | ✓ | ✓ (Metal opt-in) | ✓ |
| Windows x86_64 | ✓ | ✓ (named pipe) | ✓ | ✓ |

Tier 4 runs on Linux x86_64 only, gated on a release tag.

## Local developer loop

- **Fast loop**: `cargo test -p inferd-proto` after editing
  the proto crate. < 5 s.
- **Daemon loop**: `cargo test -p inferd-daemon` for
  lifecycle/queue work. < 30 s.
- **Engine loop**: `cargo test -p inferd-engine --features
  llamacpp-integration` after `INFERD_TEST_MODEL_PATH` is
  set. Minutes; only run when touching the engine adapter.
- **Full pre-push**: `cargo fmt --all && cargo clippy
  --all-targets --all-features -- -D warnings && cargo test
  --all && cargo audit && cargo deny check`. ~2 minutes
  without Tier 3.

## What is *not* tested here

- **The model's output quality.** inferd is a serving daemon;
  we do not assert that Gemma 4 produces a particular answer
  to a particular prompt. Sampling is non-deterministic at
  temperature > 0.
- **`llama.cpp` itself.** The pin (see `vendor/llama.cpp/PIN.md`)
  is the contract. Bumping the pin runs upstream's tests via
  their CI; we test our adapter, not their engine.
- **Operating-system kernel behaviour.** We assume working
  `flock`, `LockFileEx`, `SO_PEERCRED`, and named pipes. Tests
  exercise our usage of these, not their correctness.

## When tests fail

- **Tier 1, 2, or 5**: PR is blocked. Fix or revert.
- **Tier 3**: PR is blocked if the failure is in adapter code
  (not in the engine). If the failure is upstream (llama.cpp
  bug), open an issue, pin tighter, and document in the PR.
- **Tier 4**: blocks tagging, not PR merge.
- **Tier 6**: opens an issue with a reproducer.
