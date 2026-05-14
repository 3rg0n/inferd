# inferd v0.1 plan

- **Status**: draft
- **Date**: 2026-05-14
- **Scope**: v0.1 — drop-in replacement for thlibo's embedded
  `thlibod` daemon. No cloud backends, no model-proxy-gateway mode
  yet (that's v0.2).

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

No C FFI in v0.1. llamafile stays a subprocess.

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

### M2 — llamafile backend

- Implement `inferd-engine::llamafile::Llamafile` — spawns the binary
  as a subprocess, exchanges the existing stdio protocol thlibo uses
  (`{"system":...,"user":...}\n` → token lines → `<<END>>`).
- Implement the ready-poll that thlibo does (`READY` sentinel on
  stderr flips `Backend::ready()` true).
- Wire the queue: 1 active, 10 queued, `ErrFull` on overflow, ctx
  cancellation propagates.
- Streaming `Response::Token` frames back per newline.

**Exit criteria**: thlibo's full integration test suite (incl. the
~60s real-engine tests) passes against a running inferd. This is the
drop-in-replacement validation.

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

## Threat model (inherited)

Everything in thlibo's `THREAT_MODEL.md` that lives below the
middleware layer applies to inferd directly. In particular:

- #4 constant-time SHA compare.
- #5 NDJSON per-frame cap.
- #8 log redactor.
- #9 script entry TOCTOU — N/A, no scripts in inferd.
- #13 rolling log rotation.
- #14 systemd hardening directives.
- #21 lock-file symlink reject.
- #27/#28 SBOM + signed releases — match thlibo's v0.2 plan (cosign
  + CycloneDX).

Every remediation in thlibo is a port-target here. Don't forget any.

## Open questions for the implementer

1. **Admin socket?** thlibo v0.1 exposes an admin socket (0600,
   separate address) that broadcasts engine restart / ready events
   to connected admin clients. It's useful for `thlibo status` style
   commands and for debugging. Default decision: port it. Alternative:
   replace with a unix-domain event subscription on the main socket
   using a `subscribe: true` request field.

2. **Client identity enforcement.** thlibo v0.1 relies on socket ACLs
   only. For a host-wide daemon used by multiple middlewares, adding
   `SO_PEERCRED` / `GetNamedPipeClientProcessId` checks is cheap
   defence-in-depth and stops one bad middleware from impersonating
   another. Default decision: implement on Unix + Windows; skip on
   loopback TCP (which is always localhost-only and caller-identified
   by optional API key).

3. **Protocol versioning.** Add a `version: "v1"` field to the first
   frame on each connection? Or stay strict-v1 and let v2 introduce
   a new `/inferd-v2.sock` endpoint? Leaning toward the latter —
   simpler, and the migration story is "run both sockets during the
   transition window" which is clearer than in-band negotiation.
