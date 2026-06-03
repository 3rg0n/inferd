# 0019. Runtime accelerator detection via `GGML_BACKEND_DL`

- Status: accepted
- Date: 2026-05-26
- Accepted: 2026-06-03 (v0.3 install=work validation complete — all
  non-skip accelerator rows green: Windows CPU/CUDA, Linux CPU/CUDA,
  macOS Metal; see `docs/v0.3-validation.md`)

## Context

Through v0.2.4, every released `inferd-daemon` binary is a single
artifact built with `--features llamacpp` only. This produces a
**CPU + platform-BLAS** build:

- Linux x86_64 / aarch64: CPU with whatever SIMD the host supports.
- macOS aarch64: CPU plus Accelerate.framework (BLAS) auto-linked
  by the build script — but no Metal compute path.
- Windows x86_64: CPU only.

Operators who want CUDA, ROCm, Vulkan, or Metal compile the daemon
themselves with a cargo feature flag (`--features cuda`, etc.).
There is no runtime probe; `inferdctl doctor` reports
`accelerator=cpu` from `cfg!(feature = "...")` checks at compile
time, not from inspecting the host.

This was correct for v0.1 / v0.2 alpha — it kept the CI matrix
small, the artifacts tiny, and the build deterministic. It is
wrong for v0.3+ for three reasons:

1. **Local-inference performance is dominated by accelerator
   choice.** A 7B model decodes tens of tokens/sec on a discrete
   GPU and one-to-low-single-digit tokens/sec on CPU. Defaulting
   the released binary to CPU is leaving an order of magnitude of
   throughput on the table for any operator who has a GPU on the
   box.
2. **The lean-core posture (ADR 0006) does not require this.**
   ADR 0006 keeps HTTP / OpenAI-compat / web UIs out of the
   daemon. It says nothing about the engine being deliberately
   under-using hardware. Auto-selecting the strongest available
   backend is plumbing, which is exactly inferd's job.
3. **llama.cpp now ships a first-class dynamic-loader path
   (`GGML_BACKEND_DL=ON`)** specifically so consumers can build
   one binary that loads the correct backend lib at runtime. We
   should use it.

NPUs (Apple ANE, Intel Lunar Lake / Meteor Lake NPU via OpenVINO,
Qualcomm Snapdragon X via QNN, Microsoft DirectML NPU) are
deliberately **excluded** from the priority cascade. Vendor
toolchains for transformer LLM decode on NPUs in 2026 are
immature, and benchmark data shows NPU paths consistently lose to
the same chip's CPU+SIMD path on memory-bandwidth-bound matmul-
with-KV-cache workloads. NPUs were silicon-tuned for INT8 vision
convolutions, not LLM decode. Adding them to the cascade today
would route operators to a slower path than the default. We
revisit when the toolchains mature.

## Decision

inferd will adopt **runtime accelerator detection** with this
priority cascade:

```
Apple Metal       (Apple Silicon only)
NVIDIA CUDA       (Linux/Windows with CUDA-capable NVIDIA GPU)
AMD ROCm          (Linux with ROCm-capable AMD GPU)
Vulkan            (universal GPU fallback: AMD/Intel/older NVIDIA)
CPU + SIMD/BLAS   (deterministic floor — always available)
```

### Implementation outline

- **Build**: `crates/inferd-engine/build.rs` flips llama.cpp to
  `cmake -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON`. Per-
  platform CMake targets enable the relevant backend libs:
  - macOS aarch64: `-DGGML_METAL=ON`
  - Linux/Windows x86_64: `-DGGML_CUDA=ON -DGGML_VULKAN=ON`
    (and `-DGGML_HIPBLAS=ON` on Linux when ROCm SDK is present)
- **Distribution**: each release tarball ships the daemon binary
  *plus* a `backends/` directory containing the backend dylibs
  (`libggml-cuda.so`, `libggml-metal.dylib`, `libggml-vulkan.so`,
  …). Total size grows from ~40 MB to ~200-400 MB depending on
  which CUDA runtime version we vendor.
- **Detection**: at daemon startup the engine calls
  `ggml_backend_load_all()` (llama.cpp's built-in probe) and asks
  llama.cpp which backends successfully initialized. inferd's
  router-of-backends is *unchanged* (this is engine-level, below
  the `Backend` trait); the llamacpp adapter picks the strongest
  available compute path according to the cascade above.
- **Reporting**: `inferdctl doctor` and the admin socket replace
  the compile-time `accelerator=` string with a runtime-detected
  value, plus the device name and rough capability summary
  (e.g. `accelerator=cuda device="NVIDIA RTX 4090" vram_gb=24`).
- **Override**: a single `INFERD_FORCE_BACKEND=cpu|metal|cuda|
  rocm|vulkan` env var lets operators pin the choice for
  reproducibility / debugging. No CLI flag — env-only keeps the
  surface small.

### What this explicitly does *not* do

- **No NPU paths.** OpenVINO, ANE, DirectML-NPU, QNN are all out
  of scope until vendor toolchains for transformer decode catch
  up to GPU paths. Revisit annually.
- **No mid-stream backend switching.** Pick once at boot per the
  llama.cpp pattern. Mid-stream switching is the same anti-
  pattern ADR 0007 rejects for cloud routing.
- **No multi-GPU sharding.** Pick a primary device. Multi-GPU
  inference is a separate decision and not on the v0.3 list.
- **No CPU/GPU split-execution heuristics.** llama.cpp's
  `n_gpu_layers` knob still exists and operators can set it via
  config; auto-detection picks a sensible default for the
  detected device but does not try to be cleverer than the
  user's config when one is provided.
- **No subprocess engine.** Implementation stays inside the
  daemon process via FFI (ADR 0005). This ADR does *not* re-open
  the question of spawning `llama-cli`.

## Consequences

### What becomes easier

- **Out-of-box performance matches operator hardware.** Install
  v0.3 on a workstation with a 4090; first boot picks CUDA;
  generate latency drops by 5-30× without operator intervention.
- **Truthful marketing claim.** The inferd.io page can honestly
  say "auto-selects the strongest available accelerator at boot"
  rather than the v0.2.4 reality of "CPU + BLAS only unless you
  recompile."
- **Operator config stays simple.** Today operators choose
  between recompiling the daemon or accepting CPU. After v0.3,
  the daemon does the right thing on first boot; config-file
  edits remain available for the cases auto-detection gets
  wrong.

### What this costs

- **CI matrix expansion.** Per-OS / per-arch builds today are
  one job each. After this lands, each is a multi-backend build:
  CUDA toolkit on the Linux/Windows runners, Vulkan SDK
  everywhere, ROCm on Linux. Build times go from ~15 min to
  ~45-60 min per platform. Mitigated by aggressive sccache.
- **Tarball size growth.** ~40 MB → ~200-400 MB. Acceptable for
  a daemon that exists *because* it has a 5-10 GB model warm in
  RAM; the artifact is not the cost center.
- **Licensing review.** CUDA runtime redistribution requires
  bundling NVIDIA's EULA blob alongside the artifact. ROCm is
  MIT-licensed; Vulkan is similar. Metal ships in macOS so no
  redistribution. Each backend's redistribution terms must be
  reviewed before its lib lands in a tarball.
- **A 32-bit-class of bug surface.** "Backend X failed to load
  on operator Y's box" becomes a real support category. We need
  diagnostic output in `inferdctl doctor` strong enough that an
  operator can self-serve the triage: which backend libs were
  attempted, which loaded, which failed with what error, which
  was selected.

### Sequencing

This ADR scopes the *decision*, not the schedule. v0.3.0 is the
target release. The work splits across several commits:

1. CMake build flag flip + dynamic-load smoke test (one platform)
2. Per-platform backend lib production in CI
3. `inferdctl doctor` runtime-detection output
4. Tarball packaging + signing pipeline updates
5. End-to-end install=work validation on each platform with each
   accelerator class
6. Documentation + the inferd.io marketing-claim update

## Alternatives considered

- **Stay CPU-only and document the cargo-feature-build path.**
  Rejected: leaves an order of magnitude of performance on the
  table for any operator with a GPU. The whole point of inferd
  is to amortize one warm copy across the host; if that copy is
  10× slower than what the host can do, the value prop weakens.
- **Ship N flavored tarballs** (`inferd-cuda.tar.gz`,
  `inferd-vulkan.tar.gz`, `inferd-cpu.tar.gz`). Rejected:
  doubles the install-time-decision burden onto the operator
  (which one do I download?), and a typo / mismatch produces
  silent suboptimal performance. Auto-detection at boot
  consolidates the decision in the daemon where it belongs.
- **Pull operator config first, default to CPU only when nothing
  is configured.** Rejected: this is the v0.2.4 status quo;
  operators rarely configure GPU layers because they don't know
  to. Auto-detection on first boot solves the discovery problem.
- **Include OpenVINO in the cascade for Intel NPU/Arc paths.**
  Rejected for v0.3: NPU LLM-decode performance lags CPU+SIMD
  on every Intel chip currently available; Intel Arc discrete
  GPUs are better served by Vulkan than by OpenVINO. Revisit
  when OpenVINO's NPU plugin lands a competitive transformer
  path.
- **Spawn `llama-cli` as a subprocess and shuffle backend libs
  in/out of CWD per session.** Rejected hard: violates ADR 0005
  (no subprocess engines) and ADR 0014 (in-process plumbing).
  The dynamic-loader path inside `libllama` does this correctly
  in-process.

## References

- llama.cpp `GGML_BACKEND_DL` — runtime backend loading.
- ADR 0005 — supersedes the subprocess-engine option; this ADR
  preserves the in-process FFI invariant.
- ADR 0006 — lean-core posture; this ADR is consistent with it
  (engine performance ≠ ecosystem extension).
- ADR 0007 — backend routing semantics; this ADR is *below* the
  `Backend` trait (it picks an llamacpp compute path; it does
  not change the trait or router).
- ADR 0010 — narrow HTTPS exception for model bootstrap; backend
  libs are bundled in the tarball, not fetched at runtime.
- ADR 0012 — one warm model per process; unchanged. The
  accelerator chosen serves that one warm model.
