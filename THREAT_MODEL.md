# inferd threat model

- Status: skeleton; populated as code lands. v0.1 GA blocks
  until every "applies" finding is `mitigated` (with a code
  reference) or has an explicit waiver in this document.
- Last updated: 2026-05-15

## Scope

This file enumerates threats against `inferd-daemon` and its
client-facing surfaces:

- The NDJSON-over-IPC perimeter (Unix socket / Windows named
  pipe / loopback TCP).
- The configured backend(s) reachable through the `Backend`
  trait — in v0.1, the `libllama` FFI engine (ADR 0005); in
  v0.2, cloud adapters routed by ADR 0007 policy.
- The activity log (`inferd-daemon::logx`) and any artefacts
  it writes to disk.
- Single-instance lock acquisition and the daemon process
  itself.
- Ecosystem extensions consuming inferd over IPC (ADR 0006).
  Threats *inside* an extension are out of scope; threats from
  an extension being a malicious local actor are in scope.

## Findings

Status legend:
- **applies** — finding is real for inferd; mitigation must
  exist in code by GA.
- **mitigated** — finding is closed; mitigation site is
  named.
- **n/a** — finding does not apply; reason given.

### F-1. NDJSON per-frame size cap

**Description.** Without a per-frame byte limit, a malicious
local client can write an unbounded line without a newline,
exhausting the daemon's heap.

**Status.** applies.

**Mitigation.** `inferd-proto` reads frames with a bounded
reader (not auto-growing), 64 MiB cap. Exceeding the cap
returns an `error` frame with `code: "frame_too_large"` and
closes the connection. Codified in `docs/protocol-v1.md`.

### F-2. Lock-file pre-existing-symlink attack

**Description.** A malicious local user creates a symlink at
the lock path pointing at a privileged file before the daemon
starts. If the daemon opens with `O_CREAT` and follows
symlinks, it may write or truncate that file under its own
uid.

**Status.** applies.

**Mitigation.** `inferd-daemon::lock` rejects the lock path if
it pre-exists as a symlink. Open with `O_NOFOLLOW` on Unix.
Verify path is a regular file before locking.

### F-3. Activity log secret leakage

**Description.** Tokens streamed to or from the model may
contain secrets (the user's diff, env vars, tokens copy-pasted
into a prompt). If the activity log records request bodies or
response content verbatim, those secrets land on disk.

**Status.** mitigated. `crates/inferd-daemon/src/redact.rs`
runs `redact_in_place` inside `LogxWriter::write_record` before
any byte hits disk. Patterns: Authorization headers, key=value
secrets (password / api_key / token / etc.), JWTs,
`sk-`/`xox[baps]-`/`gh[posu]_`/`pat-`/`thingspat_` prefixes,
AWS `AKIA`/`ASIA`. Verified by `tests/logx.rs::injected_credential_does_not_leak_into_log`.

### F-4. Activity log unbounded growth

**Description.** Without rotation, the activity log grows
until the disk fills.

**Status.** mitigated. `crates/inferd-daemon/src/logx.rs`'s
`LogxWriter::write_record` rotates at the configured
`rotate_bytes` (default 16 MiB), cascading
`.ndjson` → `.1` → `.2` → `.3` and pruning anything beyond
`KEEP_GENERATIONS = 3`. Verified by `logx::tests::cascade_keeps_only_three_generations`.

### F-5. SHA-256 verification timing leak

**Description.** Comparing a model file's hash against an
expected hash byte-by-byte leaks information about how many
leading bytes match.

**Status.** applies.

**Mitigation.** Use `subtle::ConstantTimeEq` for the
verification compare. Codified in `inferd-engine`'s model-load
path.

### F-6. SHA-256 verification TOCTOU

**Description.** SHA-256 verification of the model file is
performed at load. If an attacker can rewrite the file
between verification and `mmap`, the engine loads
attacker-controlled bytes.

**Status.** mitigated.
`crates/inferd-engine/src/llamacpp/loader.rs::load_model`,
when `expected_sha256` is `Some`, copies the model file into
a daemon-owned `tempfile::TempDir`, then hashes the copy and
hands the copy path to `llama_model_load_from_file`. The
`NamedTempFile` is owned by the returned `ModelHandle` so the
copy persists for the lifetime of the loaded model — an
attacker rewriting the *original* path on disk after the copy
cannot affect the in-process model state. The tempdir is
deleted when the `ModelHandle` drops, after `llama_model_free`
has released the mmap.

When `expected_sha256` is `None`, the original path goes
straight to `libllama` with no copy and no hash. Operators
who do not configure a hash explicitly accept the
"operator-trusted model file" mode — daemon runs per-user, the
model file lives under the user's own control, an attacker
who can rewrite the user's model file has already won.

Verified by `loader::tests::load_model_with_wrong_hash_fails_at_hash_check`
and `load_model_with_no_hash_skips_copy_path`.

<!-- B1 verified: end-to-end real-Gemma run via
     `tests/echo_llamacpp.rs` against `~/.thlibo/models/
     gemma-4-e4b-ud-q4-k-xl.gguf`. Done frame carries
     backend="llamacpp", stop_reason in {End, Length},
     completion_tokens > 0. Full path validated. -->

### F-7. Per-caller identity (peer credentials)

**Description.** Without per-caller identity, any local
process with socket access is indistinguishable. A malicious
middleware can impersonate another for log-attribution
attacks, queue-fairness gaming, or future per-caller policy.

**Status.** mitigated (UDS + named pipe). TCP path covered
by F-8. `crates/inferd-daemon/src/peercred.rs` extracts a
`PeerIdentity` per connection and records it on the
`connection_accepted` activity-log event.

- Unix: `SO_PEERCRED` (Linux) / `LOCAL_PEERCRED` (macOS) via
  `nix::sys::socket::getsockopt(PeerCredentials)`. Returns
  uid/gid/pid.
- Windows: `GetNamedPipeClientProcessId` →
  `OpenProcessToken(TOKEN_QUERY)` →
  `GetTokenInformation(TokenUser)` →
  `ConvertSidToStringSidW`. Returns sid/pid.
- Loopback TCP: degraded `PeerIdentity::from_tcp(remote_addr)`
  — log-correlation only. Real perimeter is the API-key auth
  per F-8.

Verified by `peercred::tests` (`tcp_identity_displays_remote_addr`
unconditional; `unix_peer_credentials_self` `#[cfg(unix)]`).

### F-8. Loopback TCP exposure

**Description.** When the operator enables the loopback TCP
endpoint (e.g. for WSL or container scenarios), the daemon is
reachable by any process on the host that can bind 127.0.0.1.
Peer-credential checks (F-7) do not work on TCP.

**Status.** mitigated.

- `crates/inferd-daemon/src/auth.rs` parses the first NDJSON
  frame on every TCP connection as
  `{"type":"auth","key":"..."}` when
  `AcceptContext::expected_api_key` is set, and constant-time-
  compares with `subtle::ConstantTimeEq`.
- Missing or wrong key closes the connection silently (no
  protocol error frame; we don't confirm endpoint existence
  to anonymous probers). A `tcp_auth_rejected` warn-level
  event lands in the activity log.
- `--api-key` / `INFERD_API_KEY` CLI flag wires this in.
  `main.rs` warns at startup when `--tcp` is configured
  without a key.
- UDS / pipe transports skip this — F-7 covers them.
- Verified by `tests/auth.rs` (4 tests covering correct,
  wrong, missing, and disabled-by-config paths).

### F-9. FFI crash isolation

**Description.** ADR 0005 links `libllama` into the daemon
process. A segfault, abort, or stack-smash inside `libllama`
crashes the daemon, taking out all in-flight requests across
all backends.

**Status.** applies.

**Mitigation.** Defence in depth, no single fix:
- Validate every byte that reaches the engine through
  `inferd-proto` (frame cap, JSON shape, role enum, image
  budget enum, sampling param ranges).
- Pin a known-stable `llama.cpp` commit; bump only with a full
  integration-suite pass. See `vendor/llama.cpp/PIN.md`.
- Single-instance lock + autostart means a crashed daemon
  restarts cleanly; in-flight requests are lost. Caller retry
  per ADR 0007 covers this.
- Long-term: consider a sandboxed worker-process model behind
  the same `Backend` trait if a recurring class of crashes
  appears. v0.1 does not include this.

### F-10. Routing policy state corruption

**Description.** The router's circuit breaker (ADR 0007) is
daemon-local state. If the breaker can be tripped or reset by
a local attacker — e.g. by issuing many requests targeting a
backend in a way that inflates its failure count — the
attacker can deny service to that backend for the cooldown
window.

**Status.** applies. Dormant in v0.1 (single-backend, no-op
router); active when v0.2 lands the cloud router.

**Mitigation.**
- Breaker counts only *backend*-attributable errors, not
  client errors (malformed request → does not count).
- Cooldown windows are bounded; an attacker tripping every
  remote backend forces full local-only operation, not
  silence.
- Caller cannot select the backend (ADR 0006), so an attacker
  cannot deterministically push load at one backend.

### F-11. GBNF resource exhaustion

**Description.** `grammar` is a GBNF string forwarded
verbatim to the backend. A pathological grammar (deep
recursion, exponential alternation) could cause the engine to
spend unbounded CPU per token.

**Status.** mitigated.
`crates/inferd-engine/src/llamacpp/backend.rs::validate_grammar`
runs before `llama_sampler_init_grammar`:

- Total length ≤ `MAX_GRAMMAR_BYTES` = 64 KB. Real grammars are
  usually under 4 KB; this is a generous ceiling that catches
  obviously-abusive payloads.
- Alternation count (`|` byte count) ≤
  `MAX_GRAMMAR_ALTERNATIONS` = 4096. Each `|` multiplies the
  search space libllama walks per token; thousands of them is
  the "exponential alternation" abuse case.

This is not a full GBNF parser — operators wanting stricter
validation should sanitize at the caller side. Defence in
depth: frame cap (F-1) bounds the grammar payload at 64 MiB,
admission queue bounds concurrent work, `max_tokens` bounds
per-request work.

Verified by `grammar_tests::oversized_grammar_is_rejected` and
`grammar_tests::excessive_alternations_rejected`.

### F-12. Ecosystem extension trust boundary

**Description.** ADR 0006 establishes that HTTP, OpenAI-compat,
and similar surfaces live as separate processes that talk
NDJSON to inferd. A user installing a third-party extension
trusts that extension with whatever socket access the daemon
grants it.

**Status.** applies.

**Mitigation.**
- Extensions connect over the same socket as any other
  client; F-7 identity checks apply uniformly. An extension
  is not privileged inside the daemon.
- Operator documentation distinguishes inferd-shipped
  components from third-party extensions.
- Long-term: an ADR may propose a signed-extension allow-list
  if the ecosystem grows. Not v0.1.

### F-13. Ready-gating bypass

**Description.** If the inference socket is created before
the backend reports ready, clients connect, send requests,
and either get errors or block — exposing internal state and
risking races during initialisation.

**Status.** applies.

**Mitigation.** Inference listener is created strictly *after*
`Backend::ready()` returns true. Codified in
`inferd-daemon::lifecycle::Start` and verified by an
integration test that connects during startup and asserts
`ECONNREFUSED` (or platform equivalent) until ready.

### F-14. Subprocess regression

**Description.** ADR 0005 removed all engine subprocesses.
Any future PR that re-introduces a `Command::spawn` reopens
process-management vulnerabilities (zombie processes,
argument injection, environment leakage, unsafe
working-directory inheritance).

**Status.** applies.

**Mitigation.**
- Code review rule: every `std::process::Command` invocation
  requires an explicit reviewer sign-off and ADR justification.
- CI lint scans for `Command::new` and fails build if an
  exception is not annotated.
- v0.1 codebase has zero `Command::spawn` calls. Verify with
  `grep` before each release.

### F-15. Signed releases + SBOM

**Description.** Without signed release artefacts and an
SBOM, a tampered binary or a vulnerable transitive
dependency may ship undetected.

**Status.** mitigated. `.github/workflows/release.yml` builds
binaries on the four supported targets (linux x86_64, linux
arm64, macos arm64, windows x86_64), produces a CycloneDX
SBOM via `cargo cyclonedx`, and signs each archive with
keyless cosign (OIDC). `.github/workflows/ci.yml` runs
`cargo audit` on push to main + on schedule. The audit job
deliberately does not block PRs (Trivy supply-chain
incident, March 2026, was the cautionary tale).

### F-16. Daemon hardening directives

**Description.** A daemon running with default sysctl /
process settings is unnecessarily exposed (ptrace, core
dumps containing tokens, capabilities the daemon does not
need).

**Status.** mitigated.

- **Linux** — `packaging/systemd/inferd.service` applies
  `NoNewPrivileges`, `ProtectSystem=strict`,
  `ProtectHome=read-only`, `PrivateTmp`, `PrivateDevices`,
  `ProtectKernel{Tunables,Modules,Logs}`,
  `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`,
  `RestrictNamespaces`, `RestrictRealtime`, `RestrictSUIDSGID`,
  `LockPersonality`, `MemoryDenyWriteExecute`,
  `SystemCallFilter=@system-service`,
  `CapabilityBoundingSet=` (empty),
  `AmbientCapabilities=` (empty). Per-user variant; install at
  `~/.config/systemd/user/inferd.service`.
- **macOS** — `packaging/launchd/io.inferd.daemon.plist`
  ships as a per-user LaunchAgent (no LaunchDaemon, no root).
  Sandboxing applies when the signed bundle is installed from
  the release tarball.
- **Windows** — `packaging/windows/install.ps1` runs as
  `NT AUTHORITY\NetworkService`, sets recovery actions, and
  populates the activity-log env var. Custom service DACL
  applied via `sc.exe sdset` denies non-admin
  `SERVICE_STOP`/`SERVICE_START`/`SERVICE_PAUSE_CONTINUE`/
  `SERVICE_CHANGE_CONFIG` while preserving query rights. Closes
  the practical attack vector of a non-admin local user
  killing the daemon to bind the named-pipe path themselves.
  Narrower defence-in-depth than the Unix variants (no
  syscall filter, no namespace isolation) but not the
  weakest-link gap it was in alpha.2.
- The release workflow bundles the matching manifest into
  each per-platform archive.

## Process

- **Add a finding** when an ADR or PR introduces a new threat
  surface. Cross-link the ADR.
- **Mark a finding `mitigated`** when the mitigation is
  merged and named with a file path.
- **Mark a finding `n/a`** when re-evaluation determines it
  does not apply; explain why in the row.
- v0.1 GA blocks until every `applies` finding is
  `mitigated` or has an explicit waiver in this document.
