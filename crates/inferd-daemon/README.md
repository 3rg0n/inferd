# inferd-daemon

The binary. Owns the lifecycle, admission queue, single-instance
lock, IPC endpoints (Unix socket / Windows named pipe — no inbound
network listener, ADR 0022) — length-prefixed, type-tagged frames for
generation (ADR 0021), NDJSON for embeddings (ADR 0017) and rerank
(ADR 0027) — admin socket, activity log, model store, fetch, and
boot/shutdown flow.

One `lifecycle_*.rs` per wire surface, and each inference socket is bound
only when the configured backend advertises the matching capability, so
socket presence *is* capability discovery. One warm model per process
(ADR 0012), so a host wanting generation + embeddings + rerank runs three
daemons.

## Invariants

This crate is the security and lifecycle perimeter of inferd. The
non-negotiable invariants live in `../../context.md` under
"Invariants you must preserve". Notable ones:

- The inference socket does not exist until the backend is `ready`
  (THREAT_MODEL F-13). The admin socket is bound earlier so progress
  events are visible during bring-up.
- Single-instance lock via `std::fs::File::try_lock`; pre-existing
  symlinks at the lock path are refused (F-2).
- 64 MiB per-frame cap (F-5) — enforced on the length prefix *before*
  the payload is read on generation, and on line length for the embed
  and rerank NDJSON. Since v0.6.1 a request is additionally bounded to 32
  attachments and 128 MiB of them aggregate (F-1): the frame cap alone
  bounded one frame, and each declared attachment entitled the sender
  to another. A rerank request is bounded the same way and for the same
  reason — 256 documents, 8 MiB aggregate — because its cost is one
  forward pass per *document*, which no byte cap bounds.
- Admission is shared across every surface: one slot is one slot, so a
  saturated queue returns `queue_full` on generation, embed, and rerank
  alike.
- Bounded response writes (`--write-timeout-secs`, default 60s, F-17).
  Writes happen downstream of the admission gate, so a peer that stops
  reading would otherwise hold a generation slot indefinitely.
- Constant-time SHA-256 compare on model verification (`subtle`).
- No media codec (ADR 0016). Attachments arrive already decoded — raw
  RGB, or mono LE-f32 PCM at exactly the rate the backend advertises as
  `audio_sample_rate`. A mismatched rate is **rejected**, never
  resampled: the encoder takes no rate argument, so the wrong rate is a
  fluent wrong answer rather than an error. Decoding and resampling
  belong to the consumer (`inferd-http`, ADR 0025).
- Per-caller identity (`peercred.rs`): UID on Unix, SID on Windows.
- No subprocess engines (ADR 0005). llama.cpp linked via FFI.
- No HTTP server (ADR 0006). The narrow ADR 0010 HTTPS exception is
  for first-boot model bootstrap only.

See `crates/inferd-daemon/src/` for the implementation.
