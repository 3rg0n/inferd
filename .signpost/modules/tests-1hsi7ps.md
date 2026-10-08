---
type: Module
title: crates/inferd-engine/tests
description: 10 rust files; package crates::inferd-engine::tests::chat_template_gemma4.
resource: git://github.com/3rg0n/inferd@5324ed9b7adf36de21d22a1bce59482738837d94/crates/inferd-engine/tests
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "25" }
  - { name: exported, value: "0" }
  - { name: files, value: "10" }
  - { name: first_commit, value: "2026-05-15" }
  - { name: last_commit, value: "2026-08-09" }
  - { name: lines_added, value: "3364" }
  - { name: lines_removed, value: "199" }
  - { name: package, value: crates::inferd-engine::tests::chat_template_gemma4 }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./chat-template.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 13 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 7 }
  - { kind: imports, to: ../references/crates-io-futures-util.md, confidence: extracted, weight: 1, source: crates/inferd-engine/tests/openai_compat.rs }
  - { kind: imports, to: ../references/crates-io-inferd-engine.md, confidence: extracted, weight: 39, source: crates/inferd-engine/tests/chat_template_gemma4.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 58, source: crates/inferd-engine/tests/chat_template_gemma4.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 3, source: crates/inferd-engine/tests/chat_template_gemma4.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 5, source: crates/inferd-engine/tests/grammar_llamacpp.rs }
  - { kind: imports, to: ../references/crates-io-wiremock.md, confidence: extracted, weight: 6, source: crates/inferd-engine/tests/openai_compat.rs }
---
# crates/inferd-engine/tests

<!-- signpost:managed:summary -->
10 rust files; package crates::inferd-engine::tests::chat_template_gemma4.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
10 files:
- `crates/inferd-engine/tests/chat_template_gemma4.rs`
- `crates/inferd-engine/tests/chat_template_granite.rs`
- `crates/inferd-engine/tests/embed_llamacpp.rs`
- `crates/inferd-engine/tests/grammar_llamacpp.rs`
- `crates/inferd-engine/tests/llamacpp.rs`
- `crates/inferd-engine/tests/llamacpp_multimodal.rs`
- `crates/inferd-engine/tests/mock_backend.rs`
- `crates/inferd-engine/tests/openai_compat.rs`
- `crates/inferd-engine/tests/rerank_llamacpp.rs`
- `crates/inferd-engine/tests/tool_choice_llamacpp.rs`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×6, [crates/inferd-engine/src/llamacpp/chat_template](./chat-template.md) ×8, [clients/go](./go.md) ×4, [crates/inferd-daemon](./inferd-daemon.md) ×2, [crates/inferd-engine](./inferd-engine.md) ×8, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×13, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×7, [crates/inferd-daemon/src](./src-12r0iph.md) ×7, [crates/inferd-client/src](./src-162lily.md) ×2, [crates/inferd-engine/src](./src-1b7d94r.md) ×8, [crates/inferd-proto/src](./src-ymgvev.md) ×2, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×4, [crates/inferd-proto/tests](./tests-1sec76c.md) ×2, [crates/inferd-proto/src/v2](./v2.md) ×7

- **Imports**: [futures-util](../references/crates-io-futures-util.md) ×1, [inferd-engine](../references/crates-io-inferd-engine.md) ×39, [inferd-proto](../references/crates-io-inferd-proto.md) ×58, [serde_json](../references/crates-io-serde-json.md) ×3, [tokio-stream](../references/crates-io-tokio-stream.md) ×5, [wiremock](../references/crates-io-wiremock.md) ×6
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
