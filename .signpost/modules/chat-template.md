---
type: Module
title: crates/inferd-engine/src/llamacpp/chat_template
description: 4 rust files; 27 exported symbols; package crates::inferd-engine::src::llamacpp::chat_template::gemma4.
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/crates/inferd-engine/src/llamacpp/chat_template
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "8" }
  - { name: exported, value: "27" }
  - { name: files, value: "4" }
  - { name: first_commit, value: "2026-05-20" }
  - { name: last_commit, value: "2026-08-09" }
  - { name: lines_added, value: "2106" }
  - { name: lines_removed, value: "149" }
  - { name: package, value: crates::inferd-engine::src::llamacpp::chat_template::gemma4 }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ./llamacpp.md, confidence: extracted, weight: 1, source: crates/inferd-engine/src/llamacpp/chat_template/mod.rs }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 16, source: crates/inferd-engine/src/llamacpp/chat_template/gemma4.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 2, source: crates/inferd-engine/src/llamacpp/chat_template/gemma4.rs }
---
# crates/inferd-engine/src/llamacpp/chat_template

<!-- signpost:managed:summary -->
4 rust files; 27 exported symbols; package crates::inferd-engine::src::llamacpp::chat_template::gemma4.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
4 files:
- `crates/inferd-engine/src/llamacpp/chat_template/gemma4.rs`
- `crates/inferd-engine/src/llamacpp/chat_template/granite.rs`
- `crates/inferd-engine/src/llamacpp/chat_template/mod.rs`
- `crates/inferd-engine/src/llamacpp/chat_template/tool_grammar.rs`

- **Exports** (27): `ChatFamily`, `ChatFamily.as_str`, `ChatFamily.parse`, `ChatFamily.renderer`, `ChatRenderer`, `ChatRenderer.family`, `ChatRenderer.render`, `ChatRenderer.supports_thinking`, `ChatRenderer.supports_tools`, `ChatRenderer.tool_call_grammar`, `GGUF_KEY_ARCHITECTURE`, `GGUF_KEY_CHAT_TEMPLATE`, `GRAMMAR_ROOT`, `Gemma4Renderer`, `Gemma4Renderer.new`, `GraniteRenderer`, `GraniteRenderer.new`, `MEDIA_MARKER`, `RenderError`, `Rendered`, `ToolGrammar`, `ToolGrammar.is_lazy`, `detect_family`, `escape_literal`, `push_exclusion_rules`, `regex_escape`, `template_fingerprint`

- **Changes with**: [clients/go](./go.md) ×2, [crates/inferd-engine](./inferd-engine.md) ×2, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×3, [crates/inferd-daemon/src](./src-12r0iph.md) ×3, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×8, [crates/inferd-proto/src/v2](./v2.md) ×3

- **Imports**: [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×1, [inferd-proto](../references/crates-io-inferd-proto.md) ×16, [serde_json](../references/crates-io-serde-json.md) ×2
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
