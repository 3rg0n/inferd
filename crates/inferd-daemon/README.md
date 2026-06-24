# inferd-daemon

The binary. Owns the lifecycle, admission queue, single-instance
lock, IPC endpoints (Unix socket / Windows named pipe — no inbound
network listener, ADR 0022) — length-prefixed, type-tagged frames for
generation (ADR 0021), NDJSON for embeddings (ADR 0017) — admin socket,
activity log, model store, fetch, and boot/shutdown flow.

## Invariants

This crate is the security and lifecycle perimeter of inferd. The
non-negotiable invariants live in `../../context.md` under
"Invariants you must preserve". Notable ones:

- The inference socket does not exist until the backend is `ready`
  (THREAT_MODEL F-13). The admin socket is bound earlier so progress
  events are visible during bring-up.
- Single-instance lock via `std::fs::File::try_lock`; pre-existing
  symlinks at the lock path are refused (F-2).
- 64 MiB per-line NDJSON frame cap (F-5).
- Constant-time SHA-256 compare on model verification (`subtle`).
- Per-caller identity (`peercred.rs`): UID on Unix, SID on Windows.
- No subprocess engines (ADR 0005). llama.cpp linked via FFI.
- No HTTP server (ADR 0006). The narrow ADR 0010 HTTPS exception is
  for first-boot model bootstrap only.

See `crates/inferd-daemon/src/` for the implementation.
