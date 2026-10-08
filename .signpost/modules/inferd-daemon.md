---
type: Module
title: crates/inferd-daemon
description: 1 rust file; package crates::inferd-daemon::build.
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/crates/inferd-daemon
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "80" }
  - { name: exported, value: "0" }
  - { name: files, value: "1" }
  - { name: first_commit, value: "2026-05-14" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "338" }
  - { name: lines_removed, value: "158" }
  - { name: package, value: crates::inferd-daemon::build }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 99% }
edges:
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 61 }
  - { kind: co_changes, to: ./launchd.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 22 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 2 }
  - { kind: configures, to: ../references/crates-io-anyhow.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-async-trait.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-bytes.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-chrono.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-clap.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-inferd-engine.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-inferd-proto.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-nix.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-regex.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-serde.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-serde-json.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-sha2.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-subtle.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tempfile.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-thiserror.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tokio.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tokio-stream.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tracing.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-tracing-subscriber.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-ureq.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
  - { kind: configures, to: ../references/crates-io-windows-sys.md, confidence: extracted, source: crates/inferd-daemon/Cargo.toml }
---
# crates/inferd-daemon

<!-- signpost:managed:summary -->
1 rust file; package crates::inferd-daemon::build.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `crates/inferd-daemon/build.rs`

- **Changes with**: [clients/go](./go.md) ×5, [crates/inferd-engine](./inferd-engine.md) ×61, [packaging/launchd](./launchd.md) ×2, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×3, [crates/inferd-daemon/src](./src-12r0iph.md) ×22, [crates/inferd-client/src](./src-162lily.md) ×2, [crates/inferd-engine/src](./src-1b7d94r.md) ×3, [crates/inferd/src](./src-1kxpou4.md) ×6, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×6, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×2

- **Configures**: [anyhow](../references/crates-io-anyhow.md), [async-trait](../references/crates-io-async-trait.md), [bytes](../references/crates-io-bytes.md), [chrono](../references/crates-io-chrono.md), [clap](../references/crates-io-clap.md), [inferd-engine](../references/crates-io-inferd-engine.md), [inferd-proto](../references/crates-io-inferd-proto.md), [nix](../references/crates-io-nix.md), [regex](../references/crates-io-regex.md), [serde](../references/crates-io-serde.md), [serde_json](../references/crates-io-serde-json.md), [sha2](../references/crates-io-sha2.md), [subtle](../references/crates-io-subtle.md), [tempfile](../references/crates-io-tempfile.md), [thiserror](../references/crates-io-thiserror.md), [tokio](../references/crates-io-tokio.md), [tokio-stream](../references/crates-io-tokio-stream.md), [tracing](../references/crates-io-tracing.md), [tracing-subscriber](../references/crates-io-tracing-subscriber.md), [ureq](../references/crates-io-ureq.md), [windows-sys](../references/crates-io-windows-sys.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
