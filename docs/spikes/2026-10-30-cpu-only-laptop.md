# Spike: inferd on a CPU-only laptop (2026-10-30)

Goal: make **middleware <IPC> inferd <> model <> hardware ceiling-to-floor**
work well on the common machine with no discrete GPU, without growing the
system into something fragile. Driven by a real consumer (bofh, a local
sysadmin agent: ~11 tools, 1-3k-token prompts, up to 8 turns per task).

Host: Windows 11, Intel Core Ultra 7 165U (Meteor Lake, 2P + 8E + 2LP-E
cores / 14 threads, Arc iGPU, Intel AI Boost NPU driver 32.0.100.5540),
v0.8.0 networked release, `gemma-4-e4b` UD-Q4_K_XL. The machine has
63.5 GB of RAM; the design target is 16 GB, so memory numbers are judged
against that budget, not this box.

## What was wrong, measured

| # | Finding | Before | After | Where |
|---|---|---|---|---|
| #76 | generation context left at libllama's default 4 threads | 4.0 cores busy; first turn 56 s | 10 threads: 37 s | fixed on this branch |
| #77 | grammar sampling masks the full 262k vocab every token | `tool_choice: none` 35.2 s for 17 tokens; agent's forced final answer never finished in 600 s | 5.9 s; tool call + forced answer 92 s → 11 s | fixed on this branch |
| #78 | KV cleared every request | 60-80 s per agent turn, 50-70 s of it re-reading the prefix | — | ADR 0034 |
| #79 | model fetch can't resume, daemon exits on a dropped connection | 5 GB restarts from 0 | — | issue |
| #80 | Windows non-CUDA source build: VS generator drops `/O2` | 118 s vs release 35 s (same request) | Ninja: 30 s | issue; recipe below |
| — | release ships no Vulkan; 1.1 GB of CUDA on every Windows install | iGPU unused | — | ADRs 0031, 0033 |
| — | no memory fit check on CPU / unified memory (`autoselect.rs` `None => false`) | 31B would load on 16 GB | — | ADR 0032 |
| — | every boot re-hashes ~6.4 GB of GGUF; no warm-up | boot 12.5 s; first request 4.6 s | — | ADR 0035 |

End to end, an 8-turn bofh chat went from "times out" to 13.5 min with
#76 + #77; #78 is the remaining big lever.

## Engines on the iGPU / NPU (llama.cpp b12008 prebuilt, `llama-bench -fa 1`)

| Engine | Prompt tok/s (128 / 1024) | Generate tok/s |
|---|---|---|
| CPU, 10 threads | 58 / 50 | 11.0 |
| **Vulkan, Arc iGPU** | **139 / 140** | **10.6** |
| OpenVINO, Arc iGPU | 153 / — | 5.4 |
| OpenVINO, NPU | failed: no result in 15 min (UD-Q4_K_XL); Q4_0 compiled 22 min at ~16 GB resident then `graph_compute ... error -1` | — |

Conclusion (ADR 0033): Vulkan is the iGPU path; NPU and OpenVINO stay out
of the default cascade on this silicon. Re-test on Core Ultra Series 2
(Lunar Lake), where upstream validated E4B on NPU.

Package sizes (ADR 0031): core ~22 MiB (daemon 7.2, inferdctl 4.4,
llama/ggml 2.8, all CPU variants 7.4); `ggml-vulkan` 40 MiB; CUDA 1,115 MiB
(`ggml-cuda` 547 + cuBLAS/cuBLASLt 568); OpenVINO runtime 216 MiB
(+1.3 MiB plugin). Dead weight in the default install: projector 944 MiB,
embedding model 313 MiB, `inferd-http` 5.3 MiB.

## Reproduce

Thread count and sampling (needs a running daemon and a consumer; the
bofh repo's `crates/bofh-core/tests/live_inferd.rs` is one):

```sh
INFERD_N_THREADS=4  # restart daemon, time the same request, then unset
```

Engines, from llama.cpp's release zips (CPU, Vulkan, OpenVINO):

```sh
llama-bench -m gemma-4-E4B-it-UD-Q4_K_XL.gguf -p 128 -n 16 -fa 1 -r 2 -t 10          # CPU zip
llama-bench -m gemma-4-E4B-it-UD-Q4_K_XL.gguf -p 1024 -n 64 -fa 1 -ngl 99           # Vulkan zip
GGML_OPENVINO_DEVICE=GPU|NPU llama-bench -m ... -p 128 -n 16 -fa 1 -ngl 99 -r 1     # OpenVINO zip
```

Stop any other model consumer first: the daemon admits one generation at a
time, and a consumer's background work skews timings.

## Building `dl-backends` from source on Windows x64 (what actually works)

Needs VS Build Tools (C++ workload; it bundles CMake and Ninja) and
`libclang.dll` for bindgen (not in Build Tools; take it from an LLVM
release and point `LIBCLANG_PATH` at its folder). From a VS dev shell:

```bat
set CMAKE_GENERATOR=Ninja
set CFLAGS=/O2 /Ob2 /DNDEBUG
set CXXFLAGS=/O2 /Ob2 /DNDEBUG
set LIBCLANG_PATH=<folder with libclang.dll>
cargo build --release --bin inferd-daemon --features inferd-daemon/dl-backends
```

Without `CMAKE_GENERATOR=Ninja` the cmake crate drives the Visual Studio
generator and `CMAKE_C_FLAGS_RELEASE` comes out as
`-nologo -MD -Brepro -W0` — no `/O2`, ~3.4x slower (#80). The backend DLLs
land in `target/release/build/inferd-engine-*/out/{bin,lib}`; copy them
next to the daemon.

Fetching models behind an inspecting proxy that ignores `Range`: download
with any client, then `inferdctl import --name <name> --expect-sha256
<pin> <file>` (#79).

## What is on this branch

- `backend.rs`: #76 (`generation_threads`, `INFERD_N_THREADS`, 3 unit
  tests) and #77 (`sample_with_grammar` sample-then-verify +
  `apply_and_select`). Measured as above against the vendored llama.cpp
  `4f31eed`, the same pin as `main`.
- ADRs 0031-0035, all **Proposed**.
- `CHANGELOG.md` entries for the two fixes.

## Open questions for review

- Default thread count: all logical cores (current patch) vs physical
  P+E cores. On the 165U, 10 beat 14 by ~25 % on prompt evaluation
  because the LP-E cores stall the batch pass.
- ADR 0034's quota and owner key (SID + pid + start time): is the start
  time obtainable cheaply on all three OSes at accept time?
- ADR 0032's reserve defaults (8 GiB / 40 %) need a sanity check against
  real 16 GB machines.
