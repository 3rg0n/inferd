---
type: Module
title: crates/inferd-engine/src
description: 6 rust files; 32 exported symbols; package crates::inferd-engine::src::backend.
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/crates/inferd-engine/src
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: commits, value: "21" }
  - { name: exported, value: "32" }
  - { name: files, value: "6" }
  - { name: first_commit, value: "2026-05-15" }
  - { name: last_commit, value: "2026-10-08" }
  - { name: lines_added, value: "999" }
  - { name: lines_removed, value: "151" }
  - { name: package, value: crates::inferd-engine::src::backend }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 10 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 11 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 11 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 2 }
  - { kind: imports, to: ../references/crates-io-async-trait.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/backend.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 20, source: crates/inferd-engine/src/backend.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/backend.rs }
---
# crates/inferd-engine/src

<!-- signpost:managed:summary -->
6 rust files; 32 exported symbols; package crates::inferd-engine::src::backend.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
6 files:
- `crates/inferd-engine/src/backend.rs`
- `crates/inferd-engine/src/ffi.rs`
- `crates/inferd-engine/src/lib.rs`
- `crates/inferd-engine/src/mock.rs`
- `crates/inferd-engine/src/mtmd_ffi.rs`
- `crates/inferd-engine/src/tls.rs`

- **Exports** (32): `AcceleratorInfo`, `AcceleratorKind`, `AcceleratorKind.as_str`, `Backend`, `Backend.capabilities`, `Backend.embed`, `Backend.generate_v2`, `Backend.name`, `Backend.ready`, `Backend.rerank`, `Backend.stop`, `BackendCapabilities`, `DEFAULT_V2_MAX_TOKENS`, `EmbedError`, `EmbedResult`, `GenerateError`, `Mock`, `Mock.new`, `Mock.set_ready`, `Mock.with_config`, `MockConfig`, `MockError`, `RerankError`, `RerankOutcome`, `TlsSetupError`, `TokenEventV2`, `TokenStreamV2`, `bedrock_invoke`, `llamacpp`, `mock`, `openai_compat`, `tls`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×5, [clients/go](./go.md) ×2, [crates/inferd-daemon](./inferd-daemon.md) ×3, [crates/inferd-engine](./inferd-engine.md) ×10, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×11, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×5, [crates/inferd-daemon/src](./src-12r0iph.md) ×11, [crates/inferd-client/src](./src-162lily.md) ×6, [crates/inferd/src](./src-1kxpou4.md) ×4, [crates/inferd-proto/src](./src-ymgvev.md) ×3, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×6, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×8, [crates/inferd-proto/tests](./tests-1sec76c.md) ×3, [crates/inferd-proto/src/v2](./v2.md) ×2

- **Imports**: [async-trait](../references/crates-io-async-trait.md) ×2, [inferd-proto](../references/crates-io-inferd-proto.md) ×20, [tokio-stream](../references/crates-io-tokio-stream.md) ×2
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
