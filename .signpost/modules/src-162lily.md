---
type: Module
title: crates/inferd-client/src
description: 8 rust files; 32 exported symbols; package crates::inferd-client::src::admin.
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/crates/inferd-client/src
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "20" }
  - { name: exported, value: "32" }
  - { name: files, value: "8" }
  - { name: first_commit, value: "2026-05-18" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "2639" }
  - { name: lines_removed, value: "695" }
  - { name: package, value: crates::inferd-client::src::admin }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./common.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 13 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-fq5wh.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 2 }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 54, source: crates/inferd-client/src/embed_client.rs }
  - { kind: imports, to: ../references/crates-io-serde.md, confidence: extracted, weight: 1, source: crates/inferd-client/src/admin.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 25, source: crates/inferd-client/src/admin.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 2, source: crates/inferd-client/src/v2_client.rs }
---
# crates/inferd-client/src

<!-- signpost:managed:summary -->
8 rust files; 32 exported symbols; package crates::inferd-client::src::admin.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
8 files:
- `crates/inferd-client/src/admin.rs`
- `crates/inferd-client/src/client.rs`
- `crates/inferd-client/src/embed_client.rs`
- `crates/inferd-client/src/lib.rs`
- `crates/inferd-client/src/rerank_client.rs`
- `crates/inferd-client/src/transport.rs`
- `crates/inferd-client/src/v2_client.rs`
- `crates/inferd-client/src/wait.rs`

- **Exports** (32): `AdminClient`, `AdminClient.dial_admin_pipe`, `AdminClient.dial_admin_uds`, `AdminClient.recv`, `AdminClient.wait_ready`, `AdminClient.wrap_for_test`, `AdminError`, `AdminEvent`, `ClientError`, `ClientV2`, `ClientV2.dial_pipe`, `ClientV2.dial_uds`, `ClientV2.generate`, `ClientV2.wrap_for_test`, `EmbedClient`, `EmbedClient.dial_pipe`, `EmbedClient.dial_uds`, `EmbedClient.embed`, `EmbedClient.wrap_for_test`, `FrameStreamV2`, `RerankClient`, `RerankClient.dial_pipe`, `RerankClient.dial_uds`, `RerankClient.rerank`, `RerankClient.wrap_for_test`, `WaitError`, `default_admin_addr`, `default_embed_addr`, `default_rerank_addr`, `default_v2_addr`, `dial_and_wait_ready`, `is_transient_dial_error`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×2, [crates/inferd-daemon/tests/common](./common.md) ×2, [clients/go](./go.md) ×8, [crates/inferd-daemon](./inferd-daemon.md) ×2, [crates/inferd-engine](./inferd-engine.md) ×2, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×5, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×2, [crates/inferd-daemon/src](./src-12r0iph.md) ×13, [crates/inferd-engine/src](./src-1b7d94r.md) ×6, [crates/inferd/src](./src-1kxpou4.md) ×6, [crates/inferd-http/src](./src-fq5wh.md) ×2, [crates/inferd-proto/src](./src-ymgvev.md) ×4, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×7, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×2, [crates/inferd-proto/tests](./tests-1sec76c.md) ×3, [crates/inferd-proto/src/v2](./v2.md) ×2

- **Imports**: [inferd-proto](../references/crates-io-inferd-proto.md) ×54, [serde](../references/crates-io-serde.md) ×1, [tokio](../references/crates-io-tokio.md) ×25, [tokio-stream](../references/crates-io-tokio-stream.md) ×2
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
