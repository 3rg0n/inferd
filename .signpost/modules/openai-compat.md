---
type: Module
title: crates/inferd-engine/src/openai_compat
description: 4 rust files; 5 exported symbols; package crates::inferd-engine::src::openai_compat::adapter.
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/crates/inferd-engine/src/openai_compat
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: commits, value: "12" }
  - { name: exported, value: "5" }
  - { name: files, value: "4" }
  - { name: first_commit, value: "2026-05-21" }
  - { name: last_commit, value: "2026-10-08" }
  - { name: lines_added, value: "1298" }
  - { name: lines_removed, value: "164" }
  - { name: package, value: crates::inferd-engine::src::openai_compat::adapter }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 5 }
  - { kind: imports, to: ./src-1b7d94r.md, confidence: extracted, weight: 7, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: co_changes, to: ./src-1qcto7x.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-fq5wh.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ../references/crates-io-async-trait.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-eventsource-stream.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-futures-util.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-inferd-openai-wire.md, confidence: extracted, weight: 16, source: crates/inferd-engine/src/openai_compat/client.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 11, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-reqwest.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/openai_compat/mapper.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/openai_compat/adapter.rs }
  - { kind: imports, to: ../references/crates-io-tracing.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/openai_compat/adapter.rs }
---
# crates/inferd-engine/src/openai_compat

<!-- signpost:managed:summary -->
4 rust files; 5 exported symbols; package crates::inferd-engine::src::openai_compat::adapter.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
4 files:
- `crates/inferd-engine/src/openai_compat/adapter.rs`
- `crates/inferd-engine/src/openai_compat/client.rs`
- `crates/inferd-engine/src/openai_compat/mapper.rs`
- `crates/inferd-engine/src/openai_compat/mod.rs`

- **Exports** (5): `MapperError`, `OpenAiCompat`, `OpenAiCompat.new`, `OpenAiCompatConfig`, `OpenAiCompatError`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×8, [clients/go](./go.md) ×2, [crates/inferd-engine](./inferd-engine.md) ×5, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×5, [crates/inferd-daemon/src](./src-12r0iph.md) ×3, [crates/inferd-client/src](./src-162lily.md) ×2, [crates/inferd-engine/src](./src-1b7d94r.md) ×5, [crates/inferd-openai-wire/src](./src-1qcto7x.md) ×4, [crates/inferd-http/src](./src-fq5wh.md) ×4, [crates/inferd-proto/src](./src-ymgvev.md) ×2, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×3, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×7, [crates/inferd-proto/src/v2](./v2.md) ×3

- **Imports**: [crates/inferd-engine/src](./src-1b7d94r.md) ×7, [async-trait](../references/crates-io-async-trait.md) ×1, [eventsource-stream](../references/crates-io-eventsource-stream.md) ×1, [futures-util](../references/crates-io-futures-util.md) ×1, [inferd-openai-wire](../references/crates-io-inferd-openai-wire.md) ×16, [inferd-proto](../references/crates-io-inferd-proto.md) ×11, [reqwest](../references/crates-io-reqwest.md) ×2, [serde_json](../references/crates-io-serde-json.md) ×1, [tokio](../references/crates-io-tokio.md) ×1, [tokio-stream](../references/crates-io-tokio-stream.md) ×1, [tracing](../references/crates-io-tracing.md) ×2
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
