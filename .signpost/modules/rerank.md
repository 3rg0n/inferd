---
type: Module
title: crates/inferd-proto/src/rerank
description: 3 rust files; 11 exported symbols; package crates::inferd-proto::src::rerank.
resource: git://github.com/3rg0n/inferd@5324ed9b7adf36de21d22a1bce59482738837d94/crates/inferd-proto/src/rerank
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "1" }
  - { name: exported, value: "11" }
  - { name: files, value: "3" }
  - { name: first_commit, value: "2026-08-05" }
  - { name: last_commit, value: "2026-08-05" }
  - { name: lines_added, value: "505" }
  - { name: lines_removed, value: "0" }
  - { name: package, value: crates::inferd-proto::src::rerank }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: imports, to: ./src-ymgvev.md, confidence: extracted, weight: 1, source: crates/inferd-proto/src/rerank/request.rs }
  - { kind: imports, to: ../references/crates-io-serde.md, confidence: extracted, weight: 4, source: crates/inferd-proto/src/rerank/request.rs }
---
# crates/inferd-proto/src/rerank

<!-- signpost:managed:summary -->
3 rust files; 11 exported symbols; package crates::inferd-proto::src::rerank.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
3 files:
- `crates/inferd-proto/src/rerank/mod.rs`
- `crates/inferd-proto/src/rerank/request.rs`
- `crates/inferd-proto/src/rerank/response.rs`

- **Exports** (11): `MAX_RERANK_DOCUMENTS`, `MAX_RERANK_TOTAL_BYTES`, `RerankErrorCode`, `RerankRequest`, `RerankRequest.resolve`, `RerankResolved`, `RerankResponse`, `RerankResponse.id`, `RerankResponse.is_ok`, `RerankResult`, `RerankUsage`

- **Imports**: [crates/inferd-proto/src](./src-ymgvev.md) ×1, [serde](../references/crates-io-serde.md) ×4
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
