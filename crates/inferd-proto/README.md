# inferd-proto

Wire protocol for inferd. The authoritative spec lives at
[`../../docs/protocol-v1.md`](../../docs/protocol-v1.md). This crate
carries the `serde`-derived Rust types plus a small NDJSON
reader/writer with the 64 MiB per-frame cap.

Published to crates.io so every client language can either depend on
this crate directly (Rust clients) or auto-generate bindings from the
schema (Go/Python/TS clients).

**Status: not yet implemented** — starts in milestone M1.
