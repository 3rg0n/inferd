---
type: Module
title: crates/inferd-engine/src/llamacpp
description: 7 rust files; 52 exported symbols; package crates::inferd-engine::src::llamacpp::accelerator.
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/crates/inferd-engine/src/llamacpp
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "32" }
  - { name: exported, value: "52" }
  - { name: files, value: "7" }
  - { name: first_commit, value: "2026-05-16" }
  - { name: last_commit, value: "2026-08-09" }
  - { name: lines_added, value: "5040" }
  - { name: lines_removed, value: "757" }
  - { name: package, value: crates::inferd-engine::src::llamacpp::accelerator }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./chat-template.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ./chat-template.md, confidence: extracted, weight: 8, source: crates/inferd-engine/src/llamacpp/backend.rs }
  - { kind: co_changes, to: ./cpp.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 9 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 12 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 11 }
  - { kind: imports, to: ./src-1b7d94r.md, confidence: extracted, weight: 15, source: crates/inferd-engine/src/llamacpp/accelerator.rs }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 13 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 4 }
  - { kind: imports, to: ../references/crates-io-async-trait.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/llamacpp/backend.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 12, source: crates/inferd-engine/src/llamacpp/backend.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/llamacpp/tool_parser.rs }
  - { kind: imports, to: ../references/crates-io-sha2.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/llamacpp/loader.rs }
  - { kind: imports, to: ../references/crates-io-subtle.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/llamacpp/loader.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/llamacpp/backend.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/llamacpp/backend.rs }
  - { kind: imports, to: ../references/crates-io-tracing.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/llamacpp/backend.rs }
---
# crates/inferd-engine/src/llamacpp

<!-- signpost:managed:summary -->
7 rust files; 52 exported symbols; package crates::inferd-engine::src::llamacpp::accelerator.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
7 files:
- `crates/inferd-engine/src/llamacpp/accelerator.rs`
- `crates/inferd-engine/src/llamacpp/backend.rs`
- `crates/inferd-engine/src/llamacpp/grammar.rs`
- `crates/inferd-engine/src/llamacpp/loader.rs`
- `crates/inferd-engine/src/llamacpp/mod.rs`
- `crates/inferd-engine/src/llamacpp/mtmd.rs`
- `crates/inferd-engine/src/llamacpp/tool_parser.rs`

- **Exports** (52): `Bitmap`, `Bitmap.from_audio_f32`, `Bitmap.from_image_rgb`, `Bitmap.is_audio`, `Bitmap.set_id`, `DeviceMemory`, `GrammarError`, `InputChunk`, `InputChunk.id`, `InputChunk.kind`, `InputChunk.n_pos`, `InputChunk.n_tokens`, `InputChunkKind`, `InputChunks`, `InputChunks.get`, `InputChunks.is_empty`, `InputChunks.len`, `LlamaCpp`, `LlamaCpp.new`, `LlamaCppConfig`, `LlamaCppError`, `MmprojCaps`, `ModelHandle`, `ModelLoadError`, `Mtmd`, `Mtmd.audio_sample_rate`, `Mtmd.eval_chunks`, `Mtmd.new`, `Mtmd.supports_audio`, `Mtmd.supports_vision`, `Mtmd.tokenize`, `MtmdConfig`, `MtmdError`, `Output`, `ToolCallParser`, `ToolCallParser.finish`, `ToolCallParser.new`, `ToolCallParser.push`, `accelerator`, `backend`, `chat_template`, `default_media_marker`, `grammar`, `json_schema_to_gbnf`, `load_model`, `loader`, `mtmd`, `probe_accelerator`, `probe_mmproj_caps`, `query_device_memory_for_kind`, `tool_parser`, `verify_mmproj_sha256`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×5, [crates/inferd-engine/src/llamacpp/chat_template](./chat-template.md) ×3, [crates/inferd-engine/cpp](./cpp.md) ×2, [clients/go](./go.md) ×3, [crates/inferd-daemon](./inferd-daemon.md) ×3, [crates/inferd-engine](./inferd-engine.md) ×9, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×5, [crates/inferd-daemon/src](./src-12r0iph.md) ×12, [crates/inferd-client/src](./src-162lily.md) ×5, [crates/inferd-engine/src](./src-1b7d94r.md) ×11, [crates/inferd/src](./src-1kxpou4.md) ×3, [crates/inferd-proto/src](./src-ymgvev.md) ×3, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×5, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×13, [crates/inferd-proto/tests](./tests-1sec76c.md) ×2, [crates/inferd-proto/src/v2](./v2.md) ×4

- **Imports**: [crates/inferd-engine/src/llamacpp/chat_template](./chat-template.md) ×8, [crates/inferd-engine/src](./src-1b7d94r.md) ×15, [async-trait](../references/crates-io-async-trait.md) ×1, [inferd-proto](../references/crates-io-inferd-proto.md) ×12, [serde_json](../references/crates-io-serde-json.md) ×1, [sha2](../references/crates-io-sha2.md) ×2, [subtle](../references/crates-io-subtle.md) ×1, [tokio](../references/crates-io-tokio.md) ×1, [tokio-stream](../references/crates-io-tokio-stream.md) ×1, [tracing](../references/crates-io-tracing.md) ×2
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
