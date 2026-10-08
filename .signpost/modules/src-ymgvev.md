---
type: Module
title: crates/inferd-proto/src
description: 3 rust files; 15 exported symbols; package crates::inferd-proto::src::error.
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/crates/inferd-proto/src
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "8" }
  - { name: exported, value: "15" }
  - { name: files, value: "3" }
  - { name: first_commit, value: "2026-05-15" }
  - { name: last_commit, value: "2026-08-05" }
  - { name: lines_added, value: "779" }
  - { name: lines_removed, value: "277" }
  - { name: package, value: crates::inferd-proto::src::error }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 4 }
  - { kind: imports, to: ../references/crates-io-serde.md, confidence: extracted, weight: 5, source: crates/inferd-proto/src/error.rs }
---
# crates/inferd-proto/src

<!-- signpost:managed:summary -->
3 rust files; 15 exported symbols; package crates::inferd-proto::src::error.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
3 files:
- `crates/inferd-proto/src/error.rs`
- `crates/inferd-proto/src/frame.rs`
- `crates/inferd-proto/src/lib.rs`

- **Exports** (15): `ErrorCode`, `FrameType`, `MAX_FRAME_BYTES`, `ProtoError`, `ProtoError.to_error_code`, `RawFrame`, `decode_json_payload`, `embed`, `read_frame`, `read_lp_frame`, `rerank`, `v2`, `write_frame`, `write_lp_blob`, `write_lp_json`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×2, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×3, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×2, [crates/inferd-daemon/src](./src-12r0iph.md) ×3, [crates/inferd-client/src](./src-162lily.md) ×4, [crates/inferd-engine/src](./src-1b7d94r.md) ×3, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×3, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×2, [crates/inferd-proto/tests](./tests-1sec76c.md) ×4

- **Imports**: [serde](../references/crates-io-serde.md) ×5
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
