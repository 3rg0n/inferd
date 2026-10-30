# 0032. Model catalog, chosen by memory budget with a reserve for the user's apps

- Status: proposed
- Date: 2026-10-30
- Supersedes: 0023

## Context

ADR 0023 picks between two Gemma 4 tiers (12B if the accelerator has
>= 20 GiB total, else E4B) and checks free *accelerator* memory before
load. Three gaps:

1. **No guard on CPU or unified memory.** `query_device_memory_for_kind`
   returns `None` for the CPU, and the fit check treats `None` as "fits"
   (`autoselect.rs`: `None => false`). Nothing stops a 17.5 GiB 31B
   loading on a 16 GB laptop with no discrete GPU.
2. **No room left for the user.** The headroom (0.5-1 GiB) is sized for
   driver overhead. inferd runs on the user's own machine next to mail,
   office apps, browsers and coding agents; consuming all of RAM breaks
   those, which is worse than running a smaller model.
3. **Two tiers, Gemma only.** The family keeps growing (E2B, E4B, 12B,
   26B-A4B, 31B) and some consumers want Phi-4 variants.

## Decision

1. **A catalog, as data.** Replace the hard-coded tiers with a pinned
   manifest shipped in the release. Each entry: name, file URL at an
   immutable revision, SHA-256, size, estimated resident bytes at a
   given `n_ctx` (weights + KV + compute), capabilities (`tools`,
   `thinking`, `vision`), the chat family it needs (ADR 0026), and a
   capability rank. Initial entries (Q4, measured file sizes):

   | Model | File | Resident @8k (est.) | tools | thinking |
   |---|---|---|---|---|
   | gemma-4-31b | 17.5 GiB | ~20 GB | yes | yes |
   | gemma-4-26b-a4b | 15.8 GiB | ~18 GB | yes | yes |
   | gemma-4-12b | 6.9 GiB | ~11.8 GB | yes | yes |
   | gemma-4-e4b | 4.8 GiB | ~5.7 GB | yes | yes |
   | gemma-4-e2b | 3.0 GiB | ~4 GB | yes | yes |
   | phi-4 / phi-4-reasoning | 8.3 GiB | ~10 GB | no | no / yes |
   | phi-4-mini | 2.3 GiB | ~3.5 GB | yes | no |

   Phi entries are listed but not selectable until a Phi renderer exists
   (ADR 0026); the selector skips entries whose family has no renderer.

2. **Budget = total memory minus a reserve.** `budget = total -
   max(memory_reserve_min_gib, memory_reserve_fraction x total)`,
   defaults 8 GiB and 0.40. Which memory:
   - discrete GPU: the GPU's total minus 1.5 GiB (display), and host RAM
     still keeps its reserve for whatever spills;
   - CPU, iGPU, NPU and Apple unified memory: host RAM.

   Examples: 16 GB laptop -> 8 GB budget -> e4b; 32 GB -> ~19 GB -> 12b;
   64 GB -> ~38 GB -> 31b.

3. **Pick by total, check by free** (as 0023): the tier comes from total
   memory so a machine always chooses the same model; immediately before
   load, *available* memory (OS figure, now for CPU/unified too) must
   cover the estimate. If it does not, step down the catalog to the next
   entry that fits, and say so in status, instead of failing or loading
   something that pushes the user's apps out.

4. **Consumers state requirements.** A config field (later a wire field)
   names required and preferred capabilities, e.g. `tools` required,
   `thinking` preferred. The pick is the highest-ranked entry that fits
   the budget *and* meets the requirements; an agent never gets a model
   without tool calling.

5. **Text-only by default.** The vision/audio projector and the
   embedding model are not loaded unless a consumer needs them; each
   costs resident memory the whole time the daemon runs.

## Consequences

**Easier:** one rule covers laptops, desktops and GPU hosts; adding a
model is a manifest entry; the user's apps keep a known share of RAM.

**Harder / costs:** resident estimates must be measured per entry and
kept honest; the reserve is a policy the operator may need to tune
(`memory_reserve_min_gib`, `memory_reserve_fraction`).

**Explicitly NOT changed:** one warm generation model per process (ADR
0012); an explicit `backends[]` entry still overrides the selector.

**Not decided here:** unloading the model after an idle period to give
RAM back, and switching models at runtime.

## References

- ADR 0023 (superseded), ADR 0012, ADR 0026, ADR 0031.
- `autoselect.rs` `None => false` (no fit check without a device memory
  figure).
- File sizes from the Hugging Face repos, 2026-10-30; resident figures
  for e4b/12b from `docs/benchmarks/gemma4-e4b-vs-12b.md`, others
  estimated and to be measured.
