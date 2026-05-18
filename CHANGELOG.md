# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **macOS build**: `peercred::unix::from_stream` now uses
  `sockopt::LocalPeerCred` + `sockopt::LocalPeerPid` (two separate
  `getsockopt` calls) on macOS/iOS. `sockopt::PeerCredentials`
  (`SO_PEERCRED`) is Linux/Android only in nix 0.27; the previous
  code failed to compile on macOS with an unresolved import error.

### Added

- **Shared content-addressable model store** ([ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md)).
  Models now live at `$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`
  with a `manifests/<name>.json` indirection layer and an advisory
  `locks/<name>.lock` per writer. Resolution order: `models_home`
  config field → `MODELS_HOME` env → platform default
  (`%LOCALAPPDATA%\models`, `~/.local/share/models`, `~/Library/
  Application Support/models`). Wire-compatible with the cross-tool
  *Shared Local Model Store* convention so other tools that adopt
  it can share the same blobs.
- `crates/inferd-daemon/src/store.rs` — owns CAS path resolution,
  manifest read/write (atomic via `<file>.tmp` + rename), and the
  quarantine directory for SHA-mismatched bytes.

### Changed

- `crates/inferd-daemon/src/fetch.rs` — `fetch_model` now writes
  into the CAS layout: streaming download into `.partial-<hash>/
  data.tmp`, constant-time SHA verify (F-5), atomic rename into
  `<aa>/<hash>/data`, then manifest write last. Acquires
  `LOCK_EX` on `locks/<name>.lock` for the duration. The function
  signature now takes `&ModelStore` instead of `&Path`.
- `crates/inferd-daemon/src/config_file.rs` — `models_dir` field
  removed; replaced with `models_home` (optional override of
  `$MODELS_HOME`). The `model` block dropped its `filename` field
  because the on-disk path is now derived from the SHA.

### Documentation

- `README.md`, `CLAUDE.md`, `context.md`, `THREAT_MODEL.md`,
  `docs/plan-v0.1.md`, `CONTRIBUTING.md` reframed as a standalone
  service. Reference consumers (e.g. middleware projects) are
  examples of clients, not parents — inferd does not encode any
  consumer's assumptions. ADR bodies (immutable) are unchanged.

## [0.1.0-alpha.2] - 2026-05-16

Closes the three security follow-ups identified in alpha.1's
"Not yet verified" / "Post-alpha tracked work" buckets: F-7
peer credentials, F-8 TCP API-key auth, F-16 daemon hardening
manifests.

### Added

- **F-7 (peer credentials)** — `crates/inferd-daemon/src/peercred.rs`.
  `PeerIdentity` struct extracted on every accept and recorded on
  the `connection_accepted` activity-log event. Unix path uses
  `nix::sys::socket::getsockopt(PeerCredentials)`
  (`SO_PEERCRED`/`LOCAL_PEERCRED`); Windows path uses
  `GetNamedPipeClientProcessId` →
  `OpenProcessToken(TOKEN_QUERY)` →
  `GetTokenInformation(TokenUser)` →
  `ConvertSidToStringSidW`. Loopback TCP gets a degraded
  `from_tcp(remote_addr)` for log correlation; the real perimeter
  comes from F-8.
- **F-8 (TCP API key)** — `crates/inferd-daemon/src/auth.rs`.
  When `AcceptContext::expected_api_key` is `Some`, every TCP
  connection must send `{"type":"auth","key":"..."}` as its
  first NDJSON frame. Constant-time compare via
  `subtle::ConstantTimeEq`. Missing or wrong key closes the
  connection silently — no protocol error frame, no endpoint
  confirmation. New `--api-key` / `INFERD_API_KEY` flag.
- **F-16 (hardening manifests)** — `packaging/`.
  `systemd/inferd.service` (per-user, full hardening directive
  set), `launchd/io.inferd.daemon.plist` (LaunchAgent),
  `windows/install.ps1` (sc.exe with NetworkService).
  `release.yml` bundles the matching manifest into each
  per-platform release archive.
- `lifecycle::AcceptContext` struct: per-accept policy bucket
  threaded through `serve_tcp` / `serve_uds` / `serve_named_pipe`
  into `handle_connection`. Future per-connection policy (rate
  limits, per-caller quotas) extends this rather than each
  signature.

### Fixed

- `lifecycle::read_frame_async` previously wrapped its input in a
  fresh `BufReader` on every call. Bytes the fresh wrapper
  prefetched past the current line were lost when it dropped.
  Surfaced as a "request frame lost after auth" symptom in F-8
  testing. Both `read_auth_frame` and `read_frame_async` now take
  the caller's `AsyncBufRead` directly, consuming from the shared
  per-connection buffer.

### Changed

- `lifecycle::handle_connection` signature: gains `peer:
  PeerIdentity` and `ctx: AcceptContext` parameters.
  `serve_tcp` / `serve_uds` / `serve_named_pipe` likewise take
  `AcceptContext`. Tests updated.
- `crates/inferd-daemon` crate-level lint posture:
  `forbid(unsafe_code)` → `deny(unsafe_code)` so the platform-
  specific `peercred` submodules can scope an inner
  `allow(unsafe_code)` for the FFI surface. Every other module
  in the daemon remains unsafe-free.
- `windows-sys` features bumped: added
  `Win32_Security_Authorization` and `Win32_System_Memory` for
  `ConvertSidToStringSidW` and `LocalFree`.

### Security

- THREAT_MODEL F-7, F-8 → mitigated with named code sites and
  verifying tests.
- THREAT_MODEL F-16 → mitigated on Linux + macOS; Windows
  partial (service-ACL SDDL is post-alpha).
- All other findings unchanged from alpha.1.

### Verified

- 74/74 Rust tests pass on Windows (was 67 in alpha.1; +5 daemon
  unit tests under `auth::tests`, +4 integration tests in
  `tests/auth.rs`, -2 attribution).
- Workspace clippy `-D warnings` clean. fmt clean.

### Not yet verified

Same list as alpha.1 — real Gemma 4 GGUF run, CI on real
Actions runners, Linux/macOS test execution.

## [0.1.0-alpha.1] - 2026-05-16

First tagged drop. Code-complete for v0.1's planned scope plus
the pieces of M4 that landed before alpha; F-7/F-8/F-16 are the
known follow-ups (see `docs/plan-v0.1.md` §"Post-alpha tracked
work").

### Added

#### Crates

- `inferd-proto` — wire format. `Request`/`Resolved` with Gemma 4
  sampling defaults. `Response` enum with `stop_reason`,
  `backend` on `done`, structured `code` on `error` per ADR
  0008. `read_frame` / `write_frame` with a 64 MiB bounded
  reader (THREAT_MODEL F-1 mitigated). 15 tests.
- `inferd-engine` — `Backend` async trait, `TokenEvent` /
  `TokenStream`, `GenerateError`. `mock` adapter with
  failure-mode injection. `llamacpp::LlamaCpp` adapter behind
  the `llamacpp` cargo feature: model load with constant-time
  SHA-256 verification (F-5), `llama_context` allocation, decode
  + sample loop on `spawn_blocking`, GBNF wired to
  `llama_sampler_init_grammar`, cancellation by drop. 9 default
  tests + 3 tier-3 stubs that skip without
  `INFERD_TEST_MODEL_PATH`.
- `inferd-daemon` — binary. Lifecycle, single-instance lock with
  symlink rejection (F-2), bounded admission queue (1 active +
  10 queued, non-blocking submit, `code: queue_full`), no-op
  `Router` (ADR 0007 shape ready for v0.2), UDS / loopback TCP /
  Windows named-pipe endpoints, ready-gated listener creation
  (F-13), `clap`-driven CLI. 35 unit tests + 4 + 2 + 2
  integration tests.
- `inferd-stdio` — Cargo.toml scaffold only; sources land when a
  caller needs the stdio variant.

#### Activity log

- `LogxWriter` rotating NDJSON writer (3 generations, F-4)
  with a write-time `redact_in_place` redactor (F-3) covering
  Authorization headers, key=value secrets, JWTs, AWS
  AKIA/ASIA, Slack `xox*`, GitHub `gh*_`, Cisco Things
  `pat-`/`thingspat_`, OpenAI `sk-`.
- `LogxLayer` `tracing_subscriber::Layer` serialising events as
  NDJSON (`t`, `level`, `component`, `msg`, structured fields).
- `lifecycle::handle_connection` emits `request_done` /
  `request_error_mid_stream` events per request.

#### Build + release

- `vendor/llama.cpp` submodule pinned at tag `b9159` (commit
  `5c0e94683`, 2026-05-15).
- `inferd-engine/build.rs` runs CMake on the submodule under
  feature `llamacpp` with server/CLI/examples/tools/curl off,
  static-lib output, release CRT (Windows). Generates Rust
  bindings via bindgen 0.71. GPU backends as opt-in cargo
  features (`cuda`, `metal`, `vulkan`, `rocm`).
- `.github/workflows/ci.yml` — fmt + clippy + test on
  `[ubuntu, macos, windows]` with and without the `llamacpp`
  feature. Go-client job builds the daemon binary then runs
  `go vet` + `go test`. `cargo audit` on push-to-main +
  schedule (does not block PRs).
- `.github/workflows/release.yml` — tag-triggered (`v*`) matrix
  build (linux x86_64, linux aarch64 via `cross`, macos
  aarch64, windows x86_64). Generates CycloneDX SBOM via
  `cargo cyclonedx`, signs each archive with keyless cosign
  (Sigstore OIDC), publishes to GitHub Release. F-15 mitigated.

#### Go client (M5)

- `clients/go/` Go module at
  `github.com/3rg0n/inferd/clients/go`. `Client` struct with
  `DialTCP`, `DialUDS` (Unix-only), `DialPipe` (Windows-only).
  `Generate(ctx, req)` returns a frame channel; `ctx` cancel
  closes the connection. Bounded reader at 64 MiB to mirror the
  Rust crate.
- `client_test.go` — protocol-shape round-trip + end-to-end
  against the live Rust daemon binary (auto-locates
  `<workspace>/target/debug/inferd-daemon[.exe]`; override with
  `INFERD_DAEMON_BIN`).

#### Documentation

- `docs/protocol-v1.md` — clean inferd-native wire spec per ADR
  0008.
- `docs/ai.internals.explained.md` — 15-component explainer of
  how local LLM serving stacks are built (standalone reference).
- `docs/test-strategy.md` — six test tiers, platform matrix,
  cargo features.
- `docs/adr/0001`–`0009` — full architectural decision record set.
  0001 superseded by 0008; 0003 superseded by 0005.
- `THREAT_MODEL.md` — 16 findings (F-1 through F-16) with
  per-finding mitigation status and code-site references.
- `CLAUDE.md` — guidance for future Claude Code sessions in
  this repository.
- `vendor/llama.cpp.PIN.md` — pinned commit + bump procedure.

### Changed

- Workspace MSRV: floor of 1.89 (uses `std::fs::File::try_lock`,
  stable since 1.89).
- ADR 0001 → `superseded by 0008`. ADR 0003 → `superseded by
  0005`. Body of each unchanged per the ADR-immutability rule.

### Security

- THREAT_MODEL F-1, F-2, F-3, F-4, F-5, F-13, F-14, F-15
  → mitigated with named code sites and verifying tests.
- F-6, F-7, F-8, F-9, F-10, F-11, F-12, F-16 → status `applies`,
  documented as accepted-risk or post-alpha follow-up. See
  `THREAT_MODEL.md` for per-finding rationale and
  `docs/plan-v0.1.md` §"Post-alpha tracked work" for the
  schedule.
- `cargo audit` reports zero advisories across 158 dependencies.

### Verified

- 67/67 Rust tests pass on Windows under default features.
- 80/80 Rust tests pass with `llamacpp-integration` enabled
  (3 tier-3 stubs skip cleanly when no GGUF is present).
- 2/2 Go tests pass, including the round-trip against the
  spawned daemon binary.
- Workspace clippy `-D warnings` clean in both feature
  configurations.

### Not yet verified

- Real Gemma 4 GGUF run (M2c handle exists; runtime smoke is the
  operator's call).
- CI workflows on real GitHub Actions runners.
- Linux + macOS test execution (Rust toolchain runs only on
  Windows so far).
- External Go consumer importing `clients/go` end-to-end.
