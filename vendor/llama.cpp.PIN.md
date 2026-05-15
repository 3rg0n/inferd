# llama.cpp pin

inferd vendors `ggerganov/llama.cpp` as a git submodule at a
specific commit. This file records the pin, the rationale, and
the bump procedure.

## Current pin

- **Tag**: `b9159`
- **Commit**: `5c0e94683` (verify with
  `git -C vendor/llama.cpp describe --tags HEAD`)
- **Date pinned**: 2026-05-15
- **Repo**: <https://github.com/ggerganov/llama.cpp>
- **Reason for this pin**: most recent tagged build at the
  time inferd M0 closed. llama.cpp tags daily build artefacts;
  picking the most recent tag at decision time is the default
  unless a specific feature dictates otherwise.

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
