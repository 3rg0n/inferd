---
type: Module
title: crates/inferd-engine/src/bedrock_invoke
description: 5 rust files; 6 exported symbols; package crates::inferd-engine::src::bedrock_invoke::adapter.
resource: git://github.com/3rg0n/inferd@5324ed9b7adf36de21d22a1bce59482738837d94/crates/inferd-engine/src/bedrock_invoke
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "9" }
  - { name: exported, value: "6" }
  - { name: files, value: "5" }
  - { name: first_commit, value: "2026-05-21" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "2345" }
  - { name: lines_removed, value: "12" }
  - { name: package, value: crates::inferd-engine::src::bedrock_invoke::adapter }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 4 }
  - { kind: imports, to: ./src-1b7d94r.md, confidence: extracted, weight: 7, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ../references/crates-io-async-trait.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-base64.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-bytes.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/bedrock_invoke/eventstream.rs }
  - { kind: imports, to: ../references/crates-io-futures-util.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-hmac.md, confidence: extracted, weight: 3, source: crates/inferd-engine/src/bedrock_invoke/sigv4.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 11, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-reqwest.md, confidence: extracted, weight: 5, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-serde.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/bedrock_invoke/body.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/bedrock_invoke/body.rs }
  - { kind: imports, to: ../references/crates-io-sha2.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/bedrock_invoke/sigv4.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
  - { kind: imports, to: ../references/crates-io-tracing.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/bedrock_invoke/adapter.rs }
---
# crates/inferd-engine/src/bedrock_invoke

<!-- signpost:managed:summary -->
5 rust files; 6 exported symbols; package crates::inferd-engine::src::bedrock_invoke::adapter.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
5 files:
- `crates/inferd-engine/src/bedrock_invoke/adapter.rs`
- `crates/inferd-engine/src/bedrock_invoke/body.rs`
- `crates/inferd-engine/src/bedrock_invoke/eventstream.rs`
- `crates/inferd-engine/src/bedrock_invoke/mod.rs`
- `crates/inferd-engine/src/bedrock_invoke/sigv4.rs`

- **Exports** (6): `BedrockAuth`, `BedrockInvoke`, `BedrockInvoke.new`, `BedrockInvokeConfig`, `BedrockInvokeError`, `BodyError`

- **Changes with**: [clients/go](./go.md) ×2, [crates/inferd-engine](./inferd-engine.md) ×4, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×5, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×7, [crates/inferd-daemon/src](./src-12r0iph.md) ×3, [crates/inferd-client/src](./src-162lily.md) ×2, [crates/inferd-engine/src](./src-1b7d94r.md) ×4, [crates/inferd-proto/src](./src-ymgvev.md) ×2, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×3, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×6, [crates/inferd-proto/src/v2](./v2.md) ×3

- **Imports**: [crates/inferd-engine/src](./src-1b7d94r.md) ×7, [async-trait](../references/crates-io-async-trait.md) ×1, [base64](../references/crates-io-base64.md) ×2, [bytes](../references/crates-io-bytes.md) ×2, [futures-util](../references/crates-io-futures-util.md) ×1, [hmac](../references/crates-io-hmac.md) ×3, [inferd-proto](../references/crates-io-inferd-proto.md) ×11, [reqwest](../references/crates-io-reqwest.md) ×5, [serde](../references/crates-io-serde.md) ×2, [serde_json](../references/crates-io-serde-json.md) ×2, [sha2](../references/crates-io-sha2.md) ×2, [tokio](../references/crates-io-tokio.md) ×1, [tokio-stream](../references/crates-io-tokio-stream.md) ×1, [tracing](../references/crates-io-tracing.md) ×2
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
