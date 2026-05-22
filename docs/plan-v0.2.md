# inferd v0.2 plan

- **Status**: in flight on `v0.2-dev` branch
- **Date**: 2026-05-20
- **Scope**: v0.2 — gateway shape (ADR 0013), v2 wire protocol (ADR
  0015), multimodal via llama.cpp `libmtmd`, tool calling, OpenAI-compat
  HTTP backend adapter. Single warm model still — Gemma 4 only.
- **Posture**: still lean core (ADR 0006). The daemon adds *model-aware
  shaping* (chat templates, attachment routing, tool-call orchestration)
  but does not become a product. HTTP transport is still out — only
  the OpenAI-compat *backend* (outbound, behind the `Backend` trait)
  is in scope.

## Goal

Make inferd a real gateway. Middleware sends semantic intent (typed
content blocks, attachments, tool definitions); the daemon shapes that
into the format Gemma 4 expects via libllama + libmtmd; tokens come
back as v2 response frames. Same NDJSON-over-IPC transport, separate
socket path (`infer.v2.sock` / `\\.\pipe\inferd-infer-v2`).

## Non-goals for v0.2

- HTTP/gRPC transport in the daemon. v1 ban (ADR 0006) extends to v2.
- Multi-model warm pool (ADR 0012 still holds). One warm model per
  inferd process. Per the user: **Gemma 4 only** for v0.2 — no model
  registry, no selector, no fallback table. The `Backend` trait stays
  generic but the only adapter shipped configures itself for Gemma 4.
- Function-calling-as-protocol-extension on v1. Tool support is a v2
  feature; v1 stays exactly as it shipped.
- Mid-stream failover (ADR 0007). Caller still owns retry.

## Scope

### Gemma 4 only

The user has scoped v0.2 to Gemma 4 explicitly. That removes a stack of
otherwise-needed surface:

- No model registry / catalogue. The CAS store (ADR 0011) holds blobs;
  `~/.inferd/config.json` names exactly one.
- No projector type negotiation — the LlamaCpp adapter assumes Gemma 4's
  vision (`PROJECTOR_TYPE_GEMMA4V`) and audio (`PROJECTOR_TYPE_GEMMA4A`)
  projectors when an mmproj is provided.
- No chat-template selector — Gemma 4's template (`<|turn>...<turn|>`,
  `<|image|>`, `<|audio|>`, `<|tool_call>...<tool_call|>`, `<|think|>`)
  is the template the daemon emits.
- Capability flags on the Backend trait still get plumbed (Phase 2A) so
  middleware can introspect what the running daemon supports, but for
  v0.2 the answer is always "Gemma 4 + vision + audio + tools + thinking".

If a future inferd needs to serve a different model, it gets a different
backend adapter. The trait makes that possible; v0.2 does not exercise
it.

## Phase 0 — verify mtmd Gemma 4 readiness

**Status**: complete (2026-05-20).

Vendored llama.cpp pin (`vendor/llama.cpp` at b9159 / commit 5c0e94683)
ships full Gemma 4 multimodal support:

- `tools/mtmd/models/gemma4v.cpp` — vision projector graph builder
  (real implementation; SigLIP-style patch embed + position lookup +
  transformer blocks)
- `tools/mtmd/models/gemma4a.cpp` — audio Conformer encoder
  (subsampling Conv2D + dual half-step FFN + self-attention with
  sinusoidal RPE + depthwise light conv + output projection)
- `tools/mtmd/clip-impl.h` — `PROJECTOR_TYPE_GEMMA4V` and
  `PROJECTOR_TYPE_GEMMA4A` enum values + name-to-type registration
- `tools/mtmd/clip.cpp` — Gemma 4 cases in every dispatch table
  (12 separate `case PROJECTOR_TYPE_GEMMA4{V,A}:` arms across model
  load, KV graph build, encode, sizing, etc.)

The mtmd C ABI (`tools/mtmd/mtmd.h`) is what we'll FFI:

- `mtmd_init_from_file(mmproj, text_model, params)` — initialise
  multimodal context against an already-loaded text model
- `mtmd_bitmap_init(nx, ny, rgb_data)` and
  `mtmd_bitmap_init_from_audio(n_samples, f32_data)` — wrap caller's
  decoded media
- `mtmd_tokenize(ctx, output_chunks, text_with_<__media__>_markers,
  bitmaps[], n_bitmaps)` — split prompt into text chunks + image/audio
  chunks; mtmd auto-injects per-model fences (`<start_of_image>` etc.)
- `mtmd_encode_chunk(ctx, chunk)` — run the mmproj encoder for a media
  chunk; embeddings retrieved via `mtmd_get_output_embd()`
- `mtmd_support_vision(ctx)` / `mtmd_support_audio(ctx)` — capability
  introspection at runtime
- `mtmd_get_audio_sample_rate(ctx)` — middleware needs this to know
  whether to resample inbound audio (Whisper-class models want 16 kHz;
  Gemma 4a uses its own rate which we'll learn at adapter init time)

**Bitmap inputs**: middleware decodes media before sending. Image data
is `nx × ny × 3` interleaved RGB bytes; audio is float32 PCM at the
model's native sample rate. The daemon does no decode — that's a
middleware job (per ADR 0013, middleware owns "the bytes" before they
hit the wire).

**Build wiring**: `tools/mtmd/CMakeLists.txt` defines `mtmd` as a
standalone library target depending only on `ggml`, `llama`, and
`Threads`. It's currently *not* compiled because `crates/inferd-engine/
build.rs` sets `LLAMA_BUILD_TOOLS=OFF`. Phase 2A will add
`add_subdirectory(tools/mtmd)` selectively (not flip
`LLAMA_BUILD_TOOLS=ON` — that would also build CLIs we don't want) and
generate bindgen output for `mtmd.h`.

**Header warning**: mtmd's own header explicitly states
`This API is experimental and subject to many BREAKING CHANGES`. We
pin the submodule and own the FFI shim; upstream API churn becomes a
"bump the pin, fix the FFI module" task, not a wire-protocol concern.

### Gemma 4 wire-format reference

Behaviours encoded in the existing `docs/` reference material that the
daemon must emit on Gemma 4's behalf:

- `text.function.calling.with.gemma.4.md` — tool-use schema:
  `<|tool_call>{...json...}<tool_call|>` opening/closing tokens around a
  JSON object with `name` + `arguments`. Tool *results* go back in as
  text content under a tool role.
- `thinking.mode.in.gemma.md` — reasoning trace separator: tokens
  between `<|think|>` and `<|/think|>` are reasoning, not user-visible
  output. The daemon parses these out and emits them as
  `{"type":"thinking",...}` frames separate from `{"type":"token",...}`
  frames.
- `run-gemma-content-generation-and-inferences.md` — framework /
  variant landscape (background only, not requirements).

## Phase plan

| # | Phase | Task |
|---|-------|------|
| 0 | mtmd Gemma 4 readiness | #72 (this doc) |
| 1A | proto v2 types | #73 |
| 1B | v2 socket binding (stub returns `not_implemented`) | #74 |
| 2A | Backend trait grows `generate_v2` + capabilities | #75 |
| 2B | Daemon owns chat templating | TBD |
| 3A | Multimodal — attachment routing in llamacpp adapter | TBD |
| 3B | Real Gemma 4 multimodal smoke (Tier 3 integration) | TBD |
| 4A | Tool calling — tool-use frame parsing in adapter | TBD |
| 4B | Tool result injection round-trip | TBD |
| 4C | Adapter-specific tool format wrap (Gemma 4 only) | TBD |
| 5A | OpenAI-compat HTTP backend (outbound only) | TBD |
| 5B | Real router policy (failover off, circuit breaker on) | TBD |
| 6A | CI v2 matrix update | TBD |
| 6B | INTEGRATING.md final | TBD |
| 6C | Tag v0.2.0 + publish to crates.io | TBD |

Tasks 2B onward will be created as Phase 1A/1B/2A complete and we know
what the Backend trait looks like.

## Branching

All v0.2 work lands on `v0.2-dev`. `main` stays patch-only for v0.1.x
maintenance. Final merge to main happens at the v0.2.0 tag.
