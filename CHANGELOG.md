# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3] - 2026-05-19

Release-tooling fix only. Crates unchanged from 0.1.1; **no cargo
publish.** The point of the release is to get a real-inference
binary into the aarch64-linux tarball that 0.1.1 shipped mock-only.

(0.1.2 was tagged but never released — the publish step failed on
unresolvable Action versions before any artifacts were attached.
0.1.3 lands the fix.)

### Fixed

- **aarch64-linux release tarball ships with `--features llamacpp`.**
  release.yml's aarch64 job switched from `cross` (which couldn't
  configure a foreign-target C++ toolchain for llama.cpp's cmake
  build) to GitHub's native arm64 runner (`ubuntu-24.04-arm`,
  free for public repos since January 2025). Same `cargo build
  --features llamacpp` formula as the other targets — no special
  cases.

### Changed

- **GitHub Actions versions bumped** to current latest available
  major-version tags: `actions/upload-artifact@v4` → `@v7`,
  `actions/download-artifact@v4` → `@v8`. Both Node-24, closing the
  deprecation annotations from the 0.1.1 run.
  `sigstore/cosign-installer` and `softprops/action-gh-release`
  stay at `@v3`/`@v2` respectively — those projects publish 4.x /
  3.x point releases but haven't tagged a new major-version
  float, so the floats remain on v3 / v2.
- **Go version** in CI changed from pinned `1.21` to `stable`.
  Tracks current stable; the Go module's `go 1.21` directive is
  unchanged so external Go consumers on 1.21+ still work.
- **setup-go cache disabled** for the Go client job — there's no
  `go.sum` (zero external deps) so cache had nothing to key off
  and emitted a cosmetic miss annotation on every run.

## [0.1.1] - 2026-05-19

First non-alpha release. Drops the `-alpha` suffix because:

- The release tarball now ships a binary that does real inference
  (Linux x86_64, macOS aarch64, Windows x86_64 — all with
  `--features llamacpp`). Previous alpha tarball was mock-only,
  reported by an external integrator. aarch64-linux still ships
  mock-only pending a working cross-build for the C++ toolchain.
- Cross-platform validation passed across Windows + macOS +
  Linux + WSL2 systemd, including a 50-client concurrency
  stress test, mid-stream cancellation, in-flight shutdown,
  and 200-cycle connect churn.
- The Windows named-pipe DACL is now SID-restricted at the
  kernel-object level (F-7), not relying on default
  CreateNamedPipe behaviour.
- Documented multi-model decision (ADR 0012) means there are no
  open architectural questions for v0.x.

Known gap: the admission queue defined in
`crates/inferd-daemon/src/queue.rs` is not yet wired into
`handle_connection`. Today each connection runs its request
handling inline, so the protocol-promised `queue_full` error frame
is never emitted. With the llamacpp backend, concurrent requests
serialise on the inner mutex (correct behaviour, just silent
instead of `queue_full`-fronted). Tracked for v0.1.2.

### Added

- **Edition 2024 + Rust 1.95.** Workspace migrated; let-chains
  collapsed in three sites (`config_file::expand_paths`,
  `lifecycle::handle_connection`, `store::ModelStore::open`).
- **Concurrency stress harness** at
  `crates/inferd-daemon/tests/stress.rs`. Four tests covering
  50-client saturation, mid-stream disconnect resilience,
  graceful shutdown with jobs in-flight, and accept-loop pressure.
  Uses the new `MockConfig::token_delay_ms` field so requests
  overlap on the wire.
- **ADR 0012**: one warm model per inferd process. Closes the
  multi-model question that v0.1's plan flagged as a non-goal —
  multi-model warm pooling is rejected for the foreseeable v0.x
  cadence on lean-core (ADR 0006) and protocol-cost (ADR 0008)
  grounds. Operators who need N concurrent models run N inferd
  processes. The router (ADR 0007) multiplexes *backends*, not
  *models*.

### Changed

- **release.yml builds with `--features inferd-engine/llamacpp`**
  on ubuntu-latest x86_64, macos-latest aarch64, and
  windows-latest x86_64. Closes the alpha tarball gap where the
  shipped binary couldn't run real inference. aarch64-linux still
  builds mock-only via `cross` because the cross image lacks the
  C++ toolchain configuration for foreign-target cmake.
- **systemd unit**: dropped F-16 hardening directives that need
  `CAP_SYS_ADMIN` (`PrivateTmp`, `PrivateDevices`,
  `ProtectSystem=strict`, `ProtectControlGroups`, `ProtectKernel*`,
  `RestrictNamespaces`, `MemoryDenyWriteExecute`,
  `CapabilityBoundingSet`, `AmbientCapabilities`). They fail
  unit-level validation on `systemctl --user` with
  `status=218/CAPABILITIES` because a non-root user has no
  capabilities to bound or grant. The remaining set is the maximal
  subset that works without root. A future
  `inferd.service.system` template will ship the full F-16
  hardening for system-unit deployments.

### Security

- **F-7 Windows hardening**: named pipes are now created with an
  explicit SDDL DACL (`O:<sid>G:<sid>D:P(A;;GA;;;<sid>)`) that
  grants `GENERIC_ALL` to the daemon's own user SID and nobody
  else (protected DACL, no inheritance). Closes the documented
  alpha.1 gap where the pipes relied on the default
  `CreateNamedPipe` posture (creating-user-only by accident, not
  by guarantee). Implementation:
  `crates/inferd-daemon/src/windows_security.rs::
  PipeSecurityDescriptor` plus
  `ServerOptions::create_with_security_attributes_raw` in both
  `bind_named_pipe` and `bind_admin_pipe`.

### CI

- **systemd-unit smoke job** on `ubuntu-latest`. Boots the daemon
  through `systemctl --user` with the shipped unit file, verifies
  socket modes (0600 admin, 0660 inference), drives an NDJSON
  request through the inference UDS, asserts the journal contains
  no crash-loop containment trips. Closes the WSL2-systemd gap
  flagged in the Linux runtime handoff §6.

## [0.1.0-alpha.0] - 2026-05-19

First crates.io release.

### Released

- **`inferd-proto` 0.1.0-alpha.0** on crates.io. Wire format types
  (`Request`, `Response`, `Message`, `ErrorCode`, `StopReason`),
  NDJSON framing with 64 MiB per-frame cap. Canonical schema for
  any-language clients.
- **`inferd-client` 0.1.0-alpha.0** on crates.io. NDJSON-over-IPC
  client (UDS / Windows named pipe / loopback TCP), admin event
  subscriber, retry-and-wait helpers (Pattern A passive +
  Pattern B active). Re-exports `inferd-proto` so consumers don't
  need both deps.
- Both crates pinned to `inferd-daemon 0.1.0-alpha.0` via `=`-strict
  versioning so the wire-protocol contract is enforced at the
  Cargo.lock layer.

### Fixed

- **Linux runtime path defaults**: `default_admin_addr()` (daemon
  + `inferd-client`) and `DefaultAdminAddr()` (Go client) now resolve
  the Linux admin-socket path via `$XDG_RUNTIME_DIR/inferd/admin.sock`
  with fallback chain `$HOME/.inferd/run/` → `/tmp/inferd-<uid>/`.
  The previous literal `/run/inferd/admin.sock` is root-only and
  was incompatible with `systemd --user` units (per the Linux
  runtime handoff). `docs/protocol-v1.md` now freezes the
  resolution algorithm rather than a literal path.
- **systemd unit**: `packaging/systemd/inferd.service` now passes
  `--admin-addr %t/inferd/admin.sock` explicitly, drops
  `--group inferd-users` from the default ExecStart (the group
  doesn't exist on a fresh install; default `RuntimeDirectory=`
  ownership is daemon-uid-only, which is the safer default; opt
  in for multi-user shared deployments), and adds
  `StartLimitBurst=3` / `StartLimitIntervalSec=60s` to contain
  crash-loops when assets are missing. Validated end-to-end on
  Ubuntu / WSL2: daemon comes up under `systemctl --user`,
  sockets bind at `/run/user/<uid>/inferd/{admin,infer}.sock`
  with modes `0600`/`0660`, NDJSON request returns a `done`
  frame.
- **README Linux install + WSL APE-binary advisory**: documents
  the `systemctl --user` install path and warns WSL users about
  stale Cosmopolitan-Libc binaries on `PATH` (`MZ` header tripping
  the `binfmt_misc` `WSLInterop` handler).
- **CI actions upgraded to Node 24**: `actions/checkout` → v6,
  `actions/setup-go` → v6 in both CI and release workflows.
- **Windows go e2e admin addr**: `testAdminAddr` returns a named-pipe
  path on Windows so `TestEndToEndAgainstDaemon` passes the right
  `--admin-addr` format on all three platforms.
- **llamacpp Linux link**: `build.rs` now links `-lgomp` on Linux so
  `GOMP_barrier`/`GOMP_parallel`/`omp_*` symbols from `ggml-cpu`'s
  OpenMP compilation resolve at link time.
- **llamacpp macOS link**: `build.rs` now links `ggml-blas` (static)
  and `Accelerate.framework` on macOS so `_ggml_backend_blas_reg` and
  `vDSP_*` symbols resolve.
- **Go e2e on Linux**: `TestEndToEndAgainstDaemon` now passes
  `--admin-addr` pointing to a temp-dir socket instead of relying on
  the platform default (`/run/inferd/admin.sock`), which requires root
  on Linux and caused the daemon to fail before binding its TCP port.
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
