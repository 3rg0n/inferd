# Gemma 4 E4B vs 12B — local benchmark (b9850)

A/B benchmark to inform the model-tier selection heuristic (device RAM
<17 GB → E4B, >31 GB → 12B, GPU-VRAM-if-sufficient; `n_ctx` cap ~32k,
configurable). Same daemon build (b9850, `dl-backends,cuda`, release) for
both runs — the **only variable is the model**.

## Environment

- Host: Windows 11, RTX 5080 (16302 MiB VRAM), 63.6 GiB host RAM.
- Daemon: `inferd-daemon 0.5.1` on the `feat/gemma4-12b-eval` branch
  (llama.cpp b9850), CUDA backend, `n_gpu_layers=-1`.
- Idle VRAM floor (no model loaded): **~2807 MiB** (Windows desktop /
  compositor, not inferd).
- Workload: fixed prompt set (short/medium/long gen + a thinking case),
  warmup discarded, decode tok/s measured over tokens-after-first.
  Harness: `C:/dev/tmp/inferd-12b-bench` (Go, v2 wire, `DialPipe`).

## Results (RTX 5080, 16302 MiB VRAM)

| Metric | **E4B** @8k | **12B** @8k | **12B** @32k |
|---|---|---|---|
| Decode throughput (steady) | **~156–160 tok/s** | **~91–94 tok/s** | **~53–54 tok/s** |
| TTFT (warm) | ~16–27 ms | ~67–80 ms | ~690 ms |
| VRAM used (incl. ~2.8 GB idle floor) | **8630 MiB** | **14577 MiB** | **15630 MiB** |
| Free VRAM after load | 7348 MiB | 1401 MiB | **348 MiB** |
| Daemon host RSS (working set) | 4386 MiB | 8324 MiB | — |
| Daemon private mem | 7701 MiB | 13001 MiB | — |

Note: 12B @32k VRAM/tok-s measured with a **12B-only** config (see the
embed regression below); E4B and 12B@8k configs also had the
`embeddinggemma-300m` backend loaded.

### Per-case detail (b9850, CUDA)

E4B @8k: medium 156.0 tok/s (26→59), long 159.1 (24→140), thinking 160.0
(36→384). 12B @8k: medium 92.7–93.9, long 91.1–91.2 (2 passes).
12B @32k: medium 54.2, long 53.0.

(The harness's `thinking` and 2-token `short_gen` cases produce unusable
decode figures — thinking deltas arrive batched, not streamed, so the
"first-token→done" window collapses; short_gen is too few tokens. Only
the streaming medium/long text cases give a valid decode rate.)

## Evaluation vs the model-tier selection heuristic

Proposed heuristic: device RAM <17 GB → E4B, >31 GB → 12B, GPU-VRAM-if-
sufficient; `n_ctx` cap ~32k (per-backend configurable — already is).

**Findings that refine the heuristic:**

1. **12B fits a 16 GB GPU, but the real gate is (VRAM × n_ctx), not host
   RAM.** 12B @8k = 14.6 GB used; @32k = 15.6 GB used with only **348 MiB
   free**. So on a 16 GB card, 12B @32k is the practical ceiling and
   leaves no room for a second (e.g. embed) backend. Host RAM alone
   (the 17/31 GB rule) does not capture this — a 64 GB-RAM box still
   can't run 12B @32k *plus* embed on one 16 GB GPU.

2. **Throughput cost is real:** E4B ~158 → 12B@8k ~92 (−42%) → 12B@32k
   ~53 (−66% vs E4B). And @32k TTFT rises ~10× (memory pressure squeezes
   compute buffers). So "bigger is better" isn't free — 12B@32k is
   ~3× slower per token than E4B@8k.

3. **Suggested refined rule (for the future auto-select ADR):**
   - Pick the model+context that **fits available accelerator memory
     with headroom** (leave ≥1 GB VRAM free for compute buffers +
     any co-resident backend), not by host RAM alone.
   - On a 16 GB discrete GPU: E4B (any ctx up to ~32k) or 12B@≤8k with
     an embed backend co-resident; 12B@32k only if 12B is the *sole*
     backend on that GPU.
   - Mac unified memory sidesteps the discrete-VRAM ceiling (model +
     KV share the whole pool) — 12B@32k viable on a 32 GB Mac.
   - Keep `n_ctx` a first-class input to the decision, not fixed.

## Note: misleading OOM error (NOT a b9850 embed regression)

First observed as: loading `embeddinggemma-300m` as a second backend in
the **12B @32k + embed** config died with `llama_model_load: error
loading model: invalid vector subscript` → `llama_model_load_from_file
returned null`. Initially looked like a b9850 embed-load regression.

**Isolated — it is not.** Verified on this same b9850 build:
- **E4B @8k + embed**: loads clean (`backend ready`). ✓
- **12B @8k + embed**: loads clean (VRAM 14577 MiB, 1.4 GB free). ✓
- **12B @32k alone**: loads clean (VRAM 15630 MiB, only **348 MiB free**). ✓
- **12B @32k + embed**: the embed backend fails — because there is no
  VRAM left for it.

Root cause: **GPU VRAM exhaustion**, surfaced by llama.cpp as a
misleading `invalid vector subscript` instead of a clean out-of-memory
error. The embed model itself is fine; there was simply no room. This
reinforces finding #1 (the gate is VRAM×n_ctx with headroom for
co-resident backends), and is a good example of *why* the auto-select
logic must budget VRAM before binding a second backend rather than
trusting the load to fail cleanly. A minor daemon-side improvement worth
a ticket: detect the near-full-VRAM case and emit a clear
"insufficient VRAM for backend N" message rather than passing the
cryptic llama.cpp error through.
