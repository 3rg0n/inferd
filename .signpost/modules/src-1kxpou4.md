---
type: Module
title: crates/inferd/src
description: 1 rust file; entrypoint main; package crates::inferd::src::main.
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/crates/inferd/src
tags: [entrypoint]
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: commits, value: "17" }
  - { name: entrypoints, value: main }
  - { name: exported, value: "0" }
  - { name: files, value: "1" }
  - { name: first_commit, value: "2026-05-20" }
  - { name: last_commit, value: "2026-08-08" }
  - { name: lines_added, value: "1453" }
  - { name: lines_removed, value: "194" }
  - { name: package, value: crates::inferd::src::main }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 12 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 2 }
  - { kind: imports, to: ../references/crates-io-anyhow.md, confidence: extracted, weight: 1, source: crates/inferd/src/main.rs }
  - { kind: imports, to: ../references/crates-io-clap.md, confidence: extracted, weight: 2, source: crates/inferd/src/main.rs }
  - { kind: imports, to: ../references/crates-io-inferd-client.md, confidence: extracted, weight: 2, source: crates/inferd/src/main.rs }
  - { kind: imports, to: ../references/crates-io-inferd-daemon.md, confidence: extracted, weight: 9, source: crates/inferd/src/main.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 3, source: crates/inferd/src/main.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 1, source: crates/inferd/src/main.rs }
  - { kind: imports, to: ../references/crates-io-tracing-subscriber.md, confidence: extracted, weight: 3, source: crates/inferd/src/main.rs }
---
# crates/inferd/src

<!-- signpost:managed:summary -->
1 rust file; entrypoint main; package crates::inferd::src::main.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `crates/inferd/src/main.rs`

- **Changes with**: [crates/inferd-daemon](./inferd-daemon.md) ×6, [crates/inferd-engine](./inferd-engine.md) ×4, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×3, [crates/inferd-daemon/src](./src-12r0iph.md) ×12, [crates/inferd-client/src](./src-162lily.md) ×6, [crates/inferd-engine/src](./src-1b7d94r.md) ×4, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×2

- **Imports**: [anyhow](../references/crates-io-anyhow.md) ×1, [clap](../references/crates-io-clap.md) ×2, [inferd-client](../references/crates-io-inferd-client.md) ×2, [inferd-daemon](../references/crates-io-inferd-daemon.md) ×9, [serde_json](../references/crates-io-serde-json.md) ×3, [tokio](../references/crates-io-tokio.md) ×1, [tracing-subscriber](../references/crates-io-tracing-subscriber.md) ×3
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
