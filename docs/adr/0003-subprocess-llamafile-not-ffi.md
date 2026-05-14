# 0003. Subprocess llamafile, not FFI

- Status: accepted
- Date: 2026-05-14

## Context

The natural question for a Rust inference daemon is whether to embed
llama.cpp via a Rust binding crate (`llm-chain-llama`, `candle`,
`llama-cpp-rs`) or to keep spawning Mozilla's llamafile binary as a
subprocess like thlibo does today.

## Decision

Subprocess. Keep llamafile as a child process for v0.1.

## Consequences

**Why this works:**

- llamafile is already vendored into thlibo's release distribution
  and signed/pinned via SHA-256 at build time. We don't change the
  trust chain on day one.
- The stdio protocol thlibo uses (`{"system":..., "user":...}` →
  token lines → `<<END>>`) is stable and tested. We keep the test
  harness working.
- Process isolation means a llamafile crash can't take out the whole
  daemon. The restart supervisor (ported from thlibo's `engine.go`)
  handles it.
- Cross-platform: llamafile ships binaries for every platform we
  care about. Rust FFI for llama.cpp would mean building it as part
  of our release, which multiplies CI complexity.

**Cost:**

- ~2-5ms of extra latency per generation (subprocess IPC vs in-
  process call). Not load-bearing for our use case (compression of
  tool output, not real-time streaming).
- We don't get to use llama.cpp's in-process streaming APIs
  directly; every token crosses a pipe.

## When this gets revisited

v0.3+. If inferd grows workloads where subprocess latency matters
(real-time streaming chat, high-QPS summarisation), we re-open this
and consider an FFI backend *alongside* the subprocess backend —
behind the same `Backend` trait.

## References

- thlibo `internal/daemon/engine.go` — the subprocess protocol we
  are porting.
- `docs/protocol-v1.md` §Transport.
