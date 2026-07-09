# 0023. Boot-time model auto-selection by accelerator memory

- Status: accepted
- Date: 2026-07-09

## Context

ADR 0019 gave the daemon runtime accelerator detection (pick the
strongest of Metal / CUDA / ROCm / Vulkan / CPU at boot). But *which
model* to warm has always been a manual config-file pin. Gemma 4 ships
in several sizes; the two inferd cares about are **E4B** (efficient,
~5.7 GB VRAM at `n_ctx=8192`) and the dense **12B**
(`gemma-4-12b-it-UD-Q4_K_XL`, ~14.6 GB VRAM at 8k). Local benchmarks
(`docs/benchmarks/gemma4-e4b-vs-12b.md`, RTX 5080) established:

- 12B is meaningfully better quality but ~1.7× slower (E4B ~158 tok/s vs
  12B ~92 tok/s at 8k on CUDA) and needs far more memory.
- The gate on whether 12B is usable is **accelerator memory × `n_ctx`**,
  not host RAM. On a 16 GB GPU, 12B@8k leaves only ~1.4 GB free and 12B
  @32k only ~348 MB — no room for a co-resident embed backend.
- Measured idle GPU use from ordinary desktop apps (browsers, Slack,
  Electron shells) is ~2 GB and **fluctuates** with what's open, so a
  16 GB card typically shows only ~13.9 GB *free* even though it is
  clearly "12B-class" hardware.

Operators don't discover any of this. They either hand-pin 12B and hit a
cryptic out-of-memory failure (llama.cpp surfaces GPU OOM as
`invalid vector subscript` / `llama_model_load_from_file returned null`),
or they conservatively pin E4B and leave a big GPU underused. We want the
common case — "install and it warms the right model for this hardware" —
to be zero-config, while keeping explicit pins fully supported.

## Decision

At boot, **after** the ADR 0019 accelerator probe and **before** backend
construction, optionally auto-select which single Gemma 4 generation
model to warm, based on the chosen accelerator's memory.

1. **Opt-in, backwards-compatible.** New optional top-level config field
   `model_autoselect: "auto" | "off"` (default `"off"`; `#[serde(default)]`
   so every existing config deserializes unchanged). When `"off"`, the
   `backends[]` list is used verbatim — today's behaviour. Any explicit
   `backends[]` entry **always overrides** auto-selection, so power users
   pin exact variants as before.

2. **Zero-config default models.** With `model_autoselect: "auto"` and no
   generation backend listed, the daemon synthesises the backend from
   built-in pinned defaults (URL + SHA-256 + size for E4B and 12B, each
   with its `mmproj`), fetched through the ADR 0011 CAS store. No
   hand-authored `backends[]` required for the common case.

3. **Tier gate = TOTAL accelerator memory, not free.** If the chosen
   accelerator reports **total memory ≥ 20 GiB**, warm **12B**; otherwise
   warm **E4B**. Total is stable and deterministic (the same machine
   always picks the same tier); free memory is not (it drifts with other
   apps). The 20 GiB bar (above the raw 14.6 GB 12B@8k need) leaves
   headroom for desktop overhead and a co-resident embed model, so 24 GB
   and 20 GB cards get 12B while 16 GB cards get E4B — avoiding the OOM a
   16 GB card would otherwise hit. The threshold is config-overridable
   (`model_autoselect_min_vram_gib`).

4. **Free memory gates FIT, not tier.** Immediately before each model
   load, the daemon compares the accelerator's *free* memory against an
   estimate for that model + `n_ctx` (+ mmproj). If it will not fit, the
   daemon emits a **clear, actionable** error naming the requirement, the
   available memory, and concrete remedies (reduce `n_ctx`, use E4B, set
   `INFERD_FORCE_BACKEND=cpu`) instead of passing through the cryptic
   llama.cpp OOM string.

5. **Embed co-locates, else falls back to CPU.** The generation model
   targets the chosen accelerator. The embed model (embeddinggemma-300m,
   ~300 M) co-locates on the same accelerator **unless** the free-memory
   fit check says the pair won't fit; then embed loads on **CPU** by
   forcing its per-backend `n_gpu_layers = 0` (the daemon already honours
   per-backend `n_gpu_layers`, and `0` keeps a model on CPU while the
   global accelerator stays on GPU — no new plumbing). Embed stays
   *available* rather than being dropped; only its placement changes.

6. **`n_ctx` unchanged.** Default 8192, configurable per backend
   (existing behaviour). It is a first-class input to the fit estimate.

## Consequences

**Easier:**

- First-boot "just works": a 24 GB GPU auto-warms 12B, a 16 GB or CPU-only
  box auto-warms E4B — no operator config.
- Memory-exhaustion failures become legible: a clear "insufficient
  accelerator memory for backend N" message with remedies, not a cryptic
  llama.cpp panic string.
- Embeddings keep working on memory-tight hardware (they slide to CPU)
  instead of failing to load.
- The latency/quality trade is made explicitly on the operator's behalf
  and is overridable.

**Harder / costs:**

- A hardcoded per-model VRAM estimate table (weights + KV-cache growth
  with `n_ctx` + mmproj). Estimates can drift as upstream quantisation
  changes; mitigated by the benchmark doc and per-target integration
  tests. Future work could read sizes from GGUF metadata.
- One extra memory query at boot (cheap; the probe is already cached).
- Config schema grows by two optional fields (additive only).

**Explicitly NOT changed:**

- **No multi-model pool** — selection warms exactly one generation model;
  ADR 0012 preserved.
- **No mid-stream model switching** — the tier is fixed for the process
  lifetime.
- **No wire change** — this is entirely daemon-internal; the frozen v2 /
  embed / admin surfaces (ADR 0021 / 0017 / 0009) are untouched. No
  client knows or cares which variant is warm.
- **No host-RAM tier gate** — the decision is accelerator memory, per the
  benchmark findings. (Host RAM only bounds the CPU-fallback tier.)

Extends [ADR 0019](0019-runtime-accelerator-detection-via-ggml-backend-dl.md).
Preserves [ADR 0011](0011-shared-content-addressable-model-store.md) and
[ADR 0012](0012-one-warm-model-per-inferd-process.md).

## References

- `docs/benchmarks/gemma4-e4b-vs-12b.md` — E4B vs 12B vs 12B-CPU memory /
  latency data behind the 20 GiB threshold and the embed-placement rule.
- ADR 0019 — runtime accelerator detection (the probe this hooks after).
- ADR 0011 — CAS model store (default models are pinned by SHA like any
  other).
- ADR 0012 — one-warm-model invariant (selection picks one, not a pool).
