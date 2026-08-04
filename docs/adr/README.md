# Architecture Decision Records

Cross-cutting architectural decisions are recorded here. Small
implementation choices aren't.

| # | Title | Status |
|---|---|---|
| [0001](0001-wire-protocol-inherited-from-thlibo.md) | Wire protocol v1 inherited byte-for-byte from thlibo | Superseded by 0008 |
| [0002](0002-rust-not-go.md) | Rust, not Go | Accepted |
| [0003](0003-subprocess-llamafile-not-ffi.md) | Subprocess llamafile, not FFI | Superseded by 0005 |
| [0004](0004-mit-not-apache.md) | MIT license, not Apache-2.0 | Accepted |
| [0005](0005-libllama-ffi-not-subprocess.md) | Consume libllama via FFI, not llamafile as a subprocess | Accepted |
| [0006](0006-lean-core-ecosystem-extensions.md) | Lean core, ecosystem extensions live as separate processes | Accepted |
| [0007](0007-backend-routing-and-failure-semantics.md) | Backend routing: operator policy, no in-daemon retry, no mid-stream failover | Accepted |
| [0008](0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md) | Protocol v1 designed for inferd, not derived from thlibo | Accepted; v1-as-separate-surface superseded by 0021 |
| [0009](0009-pre-m1-open-questions-resolved.md) | Pre-M1 open questions resolved (admin socket, peer creds, versioning, backend identity) | Accepted; no-in-band-versioning superseded by 0021; loopback-TCP clause superseded by 0022 |
| [0010](0010-narrow-https-exception-for-model-bootstrap.md) | Narrow HTTPS exception for first-boot model bootstrap | Accepted |
| [0011](0011-shared-content-addressable-model-store.md) | Shared content-addressable model store | Accepted |
| [0012](0012-one-warm-model-per-inferd-process.md) | One warm model per inferd process (no in-daemon multi-model pool) | Accepted |
| [0013](0013-inferd-is-the-gateway-not-the-pipe.md) | inferd is the gateway, not the pipe (daemon owns model-specific shaping) | Accepted |
| [0014](0014-inferd-cli-is-a-reference-middleware.md) | The inferd CLI is a reference middleware, not a privileged surface | Superseded by 0018 (rename only) |
| [0015](0015-v2-wire-protocol-typed-content-blocks.md) | v2 wire protocol — typed content blocks, attachments, tools | Accepted (§"v2 Attachment" amended by 0016); framing superseded by 0021 |
| [0016](0016-consumer-decodes-media-before-sending.md) | Consumer decodes media before sending — daemon stays codec-free | Accepted |
| [0017](0017-embeddings-on-a-third-socket.md) | Embeddings on a third socket — NDJSON, not HTTP | Accepted |
| [0018](0018-cli-renamed-to-inferdctl.md) | CLI binary renamed back to `inferdctl` (crates.io squat + operator disambiguation) | Accepted |
| [0019](0019-runtime-accelerator-detection-via-ggml-backend-dl.md) | Runtime accelerator detection via `GGML_BACKEND_DL` (Metal / CUDA / ROCm / Vulkan / CPU cascade, no NPU) | Accepted |
| [0020](0020-inferd-http-bridge-is-a-separate-process.md) | The HTTP/OpenAI-compat bridge is a separate process, not in the daemon (two surfaces: OpenAI-compat + native-over-network) | Accepted; open question (TCP home) resolved by 0022 |
| [0021](0021-unified-v2-wire-length-prefixed-blob-framing.md) | Unify on one generation API; length-prefixed type-tagged framing, raw BLOB media, in-band `wire_version` (v0.4) | Accepted |
| [0022](0022-no-inbound-network-listener-deprecate-loopback-tcp.md) | No inbound network listener in the daemon — deprecate loopback TCP (v0.4.0), remove in v0.5.0 | Accepted |
| [0023](0023-boot-time-model-auto-selection-by-accelerator-memory.md) | Boot-time model auto-selection by accelerator memory (total VRAM ≥20 GiB → Gemma 4 12B, else E4B; embed falls back to CPU under memory pressure) | Accepted |
| [0024](0024-wsl-relay-for-containerized-middleware.md) | Crossing a VM/container boundary is the consumer's concern — inferd ships no cross-VM bridge (consumer-owned bridging preserves app-mapping fidelity; no supported no-TCP cross-VM transport exists on WSL2; recommend daemon-per-memory-domain) | Accepted |
| [0025](0025-bridge-decodes-and-resamples-audio.md) | The bridge decodes and resamples audio; codecs stay out of the daemon (extends 0016 — `inferd-http` links symphonia/rubato and reads the required rate off the admin socket per request; MPL-2.0 confined to the unpublished bridge binary) | Accepted |
| [0026](0026-chat-renderer-registry-per-model-family.md) | Chat rendering is a registry keyed to model family, not a hardcoded Gemma renderer (resolve at load from operator config or GGUF metadata; fail the load when no renderer matches, rather than silently rendering a foreign model in Gemma's grammar; no jinja evaluator in the daemon) | Proposed |
| [0027](0027-reranking-on-a-fourth-socket.md) | Reranking on a fourth socket — cross-encoder scores, not vectors (`LLAMA_POOLING_TYPE_RANK`; own NDJSON socket bound only on capability; daemon owns query/document pair formatting; ADR 0012 not relaxed — operators run a second process; runtime config flag, not a build feature) | Accepted |
| [0028](0028-airgapped-build-profile.md) | The airgapped build is a default-on `model-fetch` feature turned *off*, not an opt-in feature turned on (cargo features cannot remove a dependency, so `--no-default-features` is the only shape where "no TLS stack linked" is provable by `cargo tree`; adds `inferdctl import`; two artifacts per platform from one code path) | Accepted |

## Writing a new ADR

- One page. Nygard short form.
- Filename `NNNN-kebab-case-title.md`, zero-padded sequential.
- Once accepted, ADRs are immutable. To revise, write a new ADR
  and set the old one's status to `superseded by NNNN`.
- Reference the ADR from the corresponding `CHANGELOG.md` entry.
