# llama.cpp pin

inferd vendors `ggerganov/llama.cpp` as a git submodule at a
specific commit. This file records the pin, the rationale, and
the bump procedure.

## Current pin

- **Tag**: `b9850`
- **Commit**: `4f31eedb0` (verify with
  `git -C vendor/llama.cpp describe --tags HEAD`)
- **Date pinned**: 2026-06-30
- **Repo**: <https://github.com/ggml-org/llama.cpp> (formerly
  `ggerganov/llama.cpp`; GitHub redirects the old URL)
- **Reason for this pin**: bump from `b9159` (2026-05-14) to pick up
  **Gemma 4 12B / "unified" variant** support (the dense
  `Gemma4UnifiedForConditionalGeneration` arch — distinct from the
  E2B/E4B `Gemma4ForConditionalGeneration` we already ran) plus ~691
  commits of bug fixes, including several Gemma 4 correctness fixes
  (channel-prefix-after-tool-response, tokenizer quirks, E4B MTP
  FlashAttention). The 12B loader floor is upstream `94a220cd674`
  (PRs #24077/#24082/#24088, 2026-06-03); `b9850` is well past it.
  Landed on `main` for the v0.6.0 line — the v0.5.x maintenance line
  (`release/0.5`) stays on the previous pin.

### Previous pins

- `b9159` / `5c0e94683` (2026-05-15 → 2026-06-30): most recent tagged
  build at M0 close; shipped v0.1–v0.5.1. Supports Gemma 4 E2B/E4B,
  not the 12B unified variant.

**Status**: live. Submodule added in M2a. After cloning, run
`git submodule update --init --recursive` to populate the
working tree at this commit.

This file lives at `vendor/llama.cpp.PIN.md` (sibling of the
submodule, not inside it) so it can be edited without
modifying the submodule's tracked tree.

## Required features at this pin

For inferd v0.1 to build cleanly against the pinned commit,
the following must hold:

- Builds with `LLAMA_BUILD_SERVER=OFF`,
  `LLAMA_BUILD_EXAMPLES=OFF`, `LLAMA_BUILD_TESTS=OFF`
  producing only `libllama` (and its dependencies — `ggml`).
- Exposes a stable C API in `include/llama.h` consumable by
  `bindgen`.
- Supports loading Gemma 4 GGUF weights (E2B, E4B variants
  required; larger variants optional).
- Supports GBNF grammar-constrained sampling.
- CPU-only build works on Linux x86_64, Linux ARM64, macOS
  ARM64, Windows x86_64 with MSVC.
- Optional features (off by default in inferd, gated behind
  cargo features):
  - CUDA (`GGML_CUDA=ON`) — Linux/Windows.
  - Metal (`GGML_METAL=ON`) — macOS.
  - Vulkan (`GGML_VULKAN=ON`) — all platforms.
  - ROCm (`GGML_HIP=ON`) — Linux.

## Bump procedure

llama.cpp ships ~10 daily-build tags per day. Bumping the pin
is a real change and follows this procedure:

1. **Pick a candidate tag**. Read the upstream changelog
   between current pin and candidate. Skim PRs touching:
   `llama.h`, `ggml.h`, GGUF format version, Gemma support,
   GBNF, sampling.
2. **Update the submodule** to the candidate.
3. **Rebuild and run the full integration suite** (M2 exit
   criteria) on every CI platform.
4. **Run the differential test**: same `Request` against
   inferd-on-old-pin and inferd-on-new-pin; assert response
   shape and (within sampling tolerances) content.
5. **Update this file** with the new tag, date, and a
   one-line summary of why the bump was taken (specific
   feature, security fix, performance, or "routine").
6. **Add a `Changed` entry to `CHANGELOG.md`** referencing the
   diff range and any user-visible behaviour changes.

ADR-grade changes — GGUF format break, GBNF semantics change,
incompatible C API rename — require a new ADR before the bump
lands.

## Rejected pins

(none yet)

## References

- ADR 0005 — decision to vendor `libllama` via FFI.
- `docs/plan-v0.1.md` M2a — when the submodule is first
  added.
