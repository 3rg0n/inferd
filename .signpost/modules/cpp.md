---
type: Module
title: crates/inferd-engine/cpp
description: 2 cpp files; 3 exported symbols.
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/crates/inferd-engine/cpp
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: commits, value: "5" }
  - { name: exported, value: "3" }
  - { name: files, value: "2" }
  - { name: first_commit, value: "2026-05-20" }
  - { name: last_commit, value: "2026-06-30" }
  - { name: lines_added, value: "387" }
  - { name: lines_removed, value: "33" }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 2 }
  - { kind: configures, to: ../references/cmake-ggml.md, confidence: extracted, source: crates/inferd-engine/cpp/CMakeLists.txt }
  - { kind: configures, to: ../references/cmake-llama.md, confidence: extracted, source: crates/inferd-engine/cpp/CMakeLists.txt }
  - { kind: configures, to: ../references/cmake-threads.md, confidence: extracted, source: crates/inferd-engine/cpp/CMakeLists.txt }
---
# crates/inferd-engine/cpp

<!-- signpost:managed:summary -->
2 cpp files; 3 exported symbols.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
2 files:
- `crates/inferd-engine/cpp/grammar_shim.cpp`
- `crates/inferd-engine/cpp/string_utils.cpp`

- **Exports** (3): `string_join`, `string_repeat`, `string_split`

- **Changes with**: [crates/inferd-engine](./inferd-engine.md) ×4, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×2

- **Configures**: [ggml](../references/cmake-ggml.md), [llama](../references/cmake-llama.md), [Threads](../references/cmake-threads.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
