---
type: Module
title: crates/inferd-openai-wire/src
description: 1 rust file; 34 exported symbols; package crates::inferd-openai-wire::src::lib.
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/crates/inferd-openai-wire/src
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: commits, value: "6" }
  - { name: exported, value: "34" }
  - { name: files, value: "1" }
  - { name: first_commit, value: "2026-07-10" }
  - { name: last_commit, value: "2026-08-10" }
  - { name: lines_added, value: "681" }
  - { name: lines_removed, value: "9" }
  - { name: package, value: crates::inferd-openai-wire::src::lib }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-fq5wh.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 2 }
  - { kind: imports, to: ../references/crates-io-serde.md, confidence: extracted, weight: 2, source: crates/inferd-openai-wire/src/lib.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 1, source: crates/inferd-openai-wire/src/lib.rs }
---
# crates/inferd-openai-wire/src

<!-- signpost:managed:summary -->
1 rust file; 34 exported symbols; package crates::inferd-openai-wire::src::lib.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `crates/inferd-openai-wire/src/lib.rs`

- **Exports** (34): `ChatChunk`, `ChatCompletion`, `ChatMessage`, `ChatRequest`, `ChunkChoice`, `ChunkDelta`, `ChunkToolCallDelta`, `ChunkToolCallFunctionDelta`, `ChunkUsage`, `CompletionChoice`, `CompletionMessage`, `ContentPart`, `EmbeddingData`, `EmbeddingVector`, `EmbeddingsRequest`, `EmbeddingsResponse`, `EmbeddingsUsage`, `ErrorBody`, `ErrorEnvelope`, `ImageUrl`, `InputAudio`, `JsonSchemaSpec`, `MessageContent`, `MessageContent.as_text`, `NamedToolChoice`, `NamedToolChoiceFunction`, `ResponseFormat`, `StreamOptions`, `ToolCallFunction`, `ToolCallReplay`, `ToolChoice`, `ToolChoiceMode`, `ToolDecl`, `ToolDeclFunction`

- **Changes with**: [crates/inferd-engine](./inferd-engine.md) ×2, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×4, [crates/inferd-http/src](./src-fq5wh.md) ×6, [crates/inferd-proto/src/v2](./v2.md) ×2

- **Imports**: [serde](../references/crates-io-serde.md) ×2, [serde_json](../references/crates-io-serde-json.md) ×1
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
