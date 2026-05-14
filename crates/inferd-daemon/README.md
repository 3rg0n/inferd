# inferd-daemon

The binary. Owns the lifecycle, admission queue, single-instance
lock, NDJSON endpoints (Unix socket / Windows named pipe / loopback
TCP), activity log, and boot/shutdown flow.

**Status: not yet implemented** — starts in milestone M1.

## Source of semantics

This crate is a direct port of
[`thlibo/internal/daemon/`](https://github.com/3rg0n/thlibo/tree/main/internal/daemon)
from Go to Rust. Every invariant listed in `../../context.md` under
"Invariants you must preserve from thlibo" MUST be honoured.
