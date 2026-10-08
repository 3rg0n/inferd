---
type: Module
title: crates/inferd-daemon/tests
description: 10 rust files; package crates::inferd-daemon::tests::echo.
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/crates/inferd-daemon/tests
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "23" }
  - { name: exported, value: "0" }
  - { name: files, value: "10" }
  - { name: first_commit, value: "2026-05-15" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "5256" }
  - { name: lines_removed, value: "2300" }
  - { name: package, value: crates::inferd-daemon::tests::echo }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./common.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 17 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 4 }
  - { kind: imports, to: ../references/crates-io-async-trait.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/tests/rerank.rs }
  - { kind: imports, to: ../references/crates-io-inferd-daemon.md, confidence: extracted, weight: 54, source: crates/inferd-daemon/tests/echo.rs }
  - { kind: imports, to: ../references/crates-io-inferd-engine.md, confidence: extracted, weight: 25, source: crates/inferd-daemon/tests/echo.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 44, source: crates/inferd-daemon/tests/echo.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/tests/logx.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 16, source: crates/inferd-daemon/tests/echo.rs }
  - { kind: imports, to: ../references/crates-io-tracing-subscriber.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/tests/logx.rs }
---
# crates/inferd-daemon/tests

<!-- signpost:managed:summary -->
10 rust files; package crates::inferd-daemon::tests::echo.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
10 files:
- `crates/inferd-daemon/tests/echo.rs`
- `crates/inferd-daemon/tests/echo_llamacpp.rs`
- `crates/inferd-daemon/tests/echo_pipe.rs`
- `crates/inferd-daemon/tests/logx.rs`
- `crates/inferd-daemon/tests/queue_full.rs`
- `crates/inferd-daemon/tests/rerank.rs`
- `crates/inferd-daemon/tests/security.rs`
- `crates/inferd-daemon/tests/stress.rs`
- `crates/inferd-daemon/tests/v2_stub.rs`
- `crates/inferd-daemon/tests/write_stall.rs`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×3, [crates/inferd-daemon/tests/common](./common.md) ×2, [clients/go](./go.md) ×5, [crates/inferd-daemon](./inferd-daemon.md) ×6, [crates/inferd-engine](./inferd-engine.md) ×4, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×5, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×3, [crates/inferd-daemon/src](./src-12r0iph.md) ×17, [crates/inferd-client/src](./src-162lily.md) ×7, [crates/inferd-engine/src](./src-1b7d94r.md) ×6, [crates/inferd/src](./src-1kxpou4.md) ×2, [crates/inferd-proto/src](./src-ymgvev.md) ×3, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×4, [crates/inferd-proto/tests](./tests-1sec76c.md) ×4, [crates/inferd-proto/src/v2](./v2.md) ×4

- **Imports**: [async-trait](../references/crates-io-async-trait.md) ×1, [inferd-daemon](../references/crates-io-inferd-daemon.md) ×54, [inferd-engine](../references/crates-io-inferd-engine.md) ×25, [inferd-proto](../references/crates-io-inferd-proto.md) ×44, [serde_json](../references/crates-io-serde-json.md) ×1, [tokio](../references/crates-io-tokio.md) ×16, [tracing-subscriber](../references/crates-io-tracing-subscriber.md) ×1
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
