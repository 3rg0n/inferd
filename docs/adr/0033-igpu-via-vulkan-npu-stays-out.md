# 0033. Integrated GPUs via Vulkan; NPU and OpenVINO stay out of the default cascade

- Status: proposed
- Date: 2026-10-30

## Context

The target class of machine — a laptop with 16 GB of RAM and no
discrete GPU — almost always has an integrated GPU and, increasingly, an
NPU. ADR 0019 put Vulkan in the cascade as the universal GPU fallback
and excluded NPUs on the grounds that NPU decode loses to CPU+SIMD. Two
things changed since: the Windows release ships no Vulkan backend at all
(ADR 0031), and llama.cpp now has an OpenVINO ggml backend
(`GGML_OPENVINO`, in the vendored tree) that lists Gemma 4 E4B as
validated on CPU, GPU and NPU (on a Core Ultra Series 2, Lunar Lake).

Measured 2026-10-30 on a Core Ultra 7 165U (Series 1, Meteor Lake:
Arc iGPU, Intel AI Boost NPU driver 32.0.100.5540), gemma-4-e4b,
llama.cpp b12008 prebuilt binaries, `llama-bench -fa 1`:

| Engine | Prompt tok/s (128 / 1024) | Generate tok/s | Notes |
|---|---|---|---|
| CPU, 10 threads | 58 / 50 | 11.0 | the floor |
| Vulkan, iGPU | 139 / 140 | 10.6 | `ggml-vulkan` 40 MiB |
| OpenVINO, iGPU | 153 / — | 5.4 | runtime 217 MiB |
| OpenVINO, NPU | — | — | UD-Q4_K_XL: no result in 15 min; Q4_0 (the NPU's preferred scheme): 22 min of graph compilation at ~16 GB resident, then `graph_compute ... failed with error -1` |

Agent prompts are long (1-3k tokens) and prompt evaluation dominates
turn time (ADR 0034), so prompt throughput is the number that matters
most; generation must not regress.

## Decision

1. **Ship Vulkan** for Windows and Linux x86_64, as ADR 0019 already
   intended, as the `vulkan` engine pack (ADR 0031). On an iGPU it reads
   prompts 2.8x faster than the CPU at 1k tokens with generation on par,
   and it frees the CPU for the user's apps while it runs.
2. **Keep the NPU out of the cascade.** On Series 1 hardware it does not
   run Gemma 4 E4B at all, and its compile step alone needs as much RAM
   as the whole target machine has. ADR 0019's reasoning stands; this
   adds measured evidence for current silicon.
3. **Keep OpenVINO out of the default install.** On the iGPU it wins
   prompt throughput by ~10 % but halves generation and costs a 217 MiB
   runtime; Vulkan is the better default for the same hardware.
   `GGML_OPENVINO` remains buildable as an opt-in `openvino` pack, never
   auto-selected, for operators on newer NPUs who measure a win.
4. **Re-measure, don't assume.** Revisit when a Series 2+ (Lunar Lake or
   later) machine is available to test, and whenever the vendored
   llama.cpp is bumped, using the same `llama-bench` invocation.

## Consequences

**Easier:** the common no-dGPU laptop gets GPU prompt processing for a
40 MiB download, on both Windows and Linux.

**Harder / costs:** the release build needs the Vulkan SDK (shader
compiler) on the Windows and Linux runners.

**Explicitly NOT changed:** the cascade order and `INFERD_FORCE_BACKEND`
(ADR 0019); Metal on macOS.

## References

- ADR 0019, ADR 0031, ADR 0034.
- `vendor/llama.cpp/docs/backend/OPENVINO.md` (validated models, NPU
  limitations: stateless only, static graphs, small context).
- llama.cpp b12008 `win-vulkan-x64`, `win-openvino-2026.4.1-x64`,
  `win-cpu-x64` release assets.
