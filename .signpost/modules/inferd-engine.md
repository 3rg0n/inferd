---
type: Module
title: crates/inferd-engine
description: 1 rust file; package crates::inferd-engine::build.
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/crates/inferd-engine
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "86" }
  - { name: exported, value: "0" }
  - { name: files, value: "1" }
  - { name: first_commit, value: "2026-05-14" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "1138" }
  - { name: lines_removed, value: "353" }
  - { name: package, value: crates::inferd-engine::build }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 99% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./chat-template.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./cpp.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 61 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 9 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 9 }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-1qcto7x.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-fq5wh.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 2 }
  - { kind: configures, to: ../references/crates-io-async-trait.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-base64.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-bindgen.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-bytes.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-cmake.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-eventsource-stream.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-futures-core.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-futures-util.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-hex.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-hmac.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-image.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-inferd-openai-wire.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-inferd-proto.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-reqwest.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-serde.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-serde-json.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-sha2.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-subtle.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tempfile.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-thiserror.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tokio.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tokio-stream.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tracing.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-url.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-wiremock.md, confidence: extracted, source: crates/inferd-engine/Cargo.toml }
---
# crates/inferd-engine

<!-- signpost:managed:summary -->
1 rust file; package crates::inferd-engine::build.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `crates/inferd-engine/build.rs`

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×4, [crates/inferd-engine/src/llamacpp/chat_template](./chat-template.md) ×2, [crates/inferd-engine/cpp](./cpp.md) ×4, [clients/go](./go.md) ×8, [crates/inferd-daemon](./inferd-daemon.md) ×61, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×9, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×4, [crates/inferd-daemon/src](./src-12r0iph.md) ×7, [crates/inferd-client/src](./src-162lily.md) ×2, [crates/inferd-engine/src](./src-1b7d94r.md) ×9, [crates/inferd/src](./src-1kxpou4.md) ×4, [crates/inferd-openai-wire/src](./src-1qcto7x.md) ×2, [crates/inferd-http/src](./src-fq5wh.md) ×2, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×4, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×8, [crates/inferd-proto/src/v2](./v2.md) ×2

- **Configures**: [async-trait](../references/crates-io-async-trait.md), [base64](../references/crates-io-base64.md), [bindgen](../references/crates-io-bindgen.md), [bytes](../references/crates-io-bytes.md), [cmake](../references/crates-io-cmake.md), [eventsource-stream](../references/crates-io-eventsource-stream.md), [futures-core](../references/crates-io-futures-core.md), [futures-util](../references/crates-io-futures-util.md), [hex](../references/crates-io-hex.md), [hmac](../references/crates-io-hmac.md), [image](../references/crates-io-image.md), [inferd-openai-wire](../references/crates-io-inferd-openai-wire.md), [inferd-proto](../references/crates-io-inferd-proto.md), [reqwest](../references/crates-io-reqwest.md), [serde](../references/crates-io-serde.md), [serde_json](../references/crates-io-serde-json.md), [sha2](../references/crates-io-sha2.md), [subtle](../references/crates-io-subtle.md), [tempfile](../references/crates-io-tempfile.md), [thiserror](../references/crates-io-thiserror.md), [tokio](../references/crates-io-tokio.md), [tokio-stream](../references/crates-io-tokio-stream.md), [tracing](../references/crates-io-tracing.md), [url](../references/crates-io-url.md), [wiremock](../references/crates-io-wiremock.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
