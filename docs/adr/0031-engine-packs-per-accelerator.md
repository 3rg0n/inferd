# 0031. Engine packs per accelerator, installed by hardware probe

- Status: proposed
- Date: 2026-10-30

## Context

ADR 0019 made the daemon load whatever ggml backend libraries sit next
to it and pick the strongest at boot (Metal > CUDA > ROCm > Vulkan >
CPU). The routing is right. The packaging is not:

- The Windows x64 release is one 652 MiB zip (1.2 GiB unpacked) built
  with `dl-backends,cuda`. 1,115 MiB of that is `ggml-cuda` (547 MiB)
  plus cuBLAS / cuBLASLt (568 MiB), which can only load on an NVIDIA
  GPU. Everything else — daemon 7.2, `inferdctl` 4.4, libllama/ggml
  2.8, all CPU kernel variants 7.4 MiB — is about 22 MiB.
- Vulkan, which ADR 0019 lists as the universal GPU fallback and plans
  as `-DGGML_CUDA=ON -DGGML_VULKAN=ON` for Linux/Windows x86_64, is not
  in the release at all. An Intel or AMD iGPU laptop — the common case
  without a discrete GPU — downloads 1.1 GB it cannot use and still runs
  on CPU. `ggml-vulkan` is 40 MiB (llama.cpp b12008, win-x64).
- Building from source to get a missing backend needs MSVC, CMake,
  Ninja and libclang (see #80); that is not a reasonable user step.

## Decision

Ship the daemon as a small **core** plus optional prebuilt **engine
packs**, one per accelerator family, as release artifacts. The
installer probes the machine and downloads only the packs it can use.
The daemon is unchanged: ADR 0019's cascade still picks the strongest of
whatever is installed.

| Pack | Contents | Size | Installed when |
|---|---|---|---|
| core | daemon, `inferdctl`, libllama, ggml-base, all CPU variants | ~22 MiB | always (the floor) |
| vulkan | `ggml-vulkan` | ~40 MiB | any GPU on Windows/Linux x86_64 that is not NVIDIA |
| cuda | `ggml-cuda`, cuBLAS, cuBLASLt, cudart | ~1.1 GiB | NVIDIA GPU present |
| openvino | `ggml-openvino` + OpenVINO runtime | ~217 MiB | Intel GPU/NPU present (ADR 0033) |
| metal | built into the macOS core | 0 | Apple Silicon |

- **Probe**: GPU vendor IDs from the OS adapter list (Windows DXGI/WMI,
  Linux `/sys/class/drm/*/device/vendor`): `10DE` NVIDIA → cuda, `8086`
  Intel → vulkan + openvino, `1002` AMD → vulkan. NPU presence from the
  OS device list. macOS arm64 needs no probe.
- **Integrity**: each pack is a separate artifact with its own SHA-256
  and cosign bundle, verified exactly like today's archive.
- **Re-probe**: `inferdctl engines sync` repeats the probe and fetches
  or removes packs when hardware changes (eGPU, driver install).
- **Airgapped** (ADR 0028): packs are plain archives; the airgapped
  build ships core and accepts packs placed by hand.
- **Not in the default install**: `inferd-http` (ADR 0020 extension,
  5.3 MiB), the vision/audio projector (944 MiB) and the embedding model
  (313 MiB) — fetched when a consumer asks for them (ADR 0032).

## Consequences

**Easier:** an iGPU laptop installs ~62 MiB and uses its GPU; an NVIDIA
host gets the same CUDA it gets today; no user ever compiles anything.

**Harder / costs:** the release job builds and publishes several
artifacts per platform instead of one, and the installer gains a probe.
A pack built from a different llama.cpp commit than core must never
load: packs are versioned with core and the daemon refuses a mismatch.

**Explicitly NOT changed:** the accelerator cascade and its
`INFERD_FORCE_BACKEND` override (ADR 0019); the wire; the model store.

## References

- ADR 0019 (cascade), ADR 0010 (pinned downloads), ADR 0028 (airgapped).
- Issue #80 (Windows source builds lose `/O2`).
- Sizes measured from the v0.8.0 Windows x64 release and llama.cpp
  b12008 Windows assets, 2026-10-30.
