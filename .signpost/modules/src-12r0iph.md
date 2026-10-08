---
type: Module
title: crates/inferd-daemon/src
description: 21 rust files; 154 exported symbols; entrypoint main; package crates::inferd-daemon::src::admin.
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/crates/inferd-daemon/src
tags: [entrypoint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "72" }
  - { name: entrypoints, value: main }
  - { name: exported, value: "154" }
  - { name: files, value: "21" }
  - { name: first_commit, value: "2026-05-15" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "13882" }
  - { name: lines_removed, value: "2909" }
  - { name: package, value: crates::inferd-daemon::src::admin }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./chat-template.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./common.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 20 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./launchd.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 12 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./packaging.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 13 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 11 }
  - { kind: co_changes, to: ./src-1kxpou4.md, confidence: extracted, weight: 12 }
  - { kind: co_changes, to: ./src-ymgvev.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 17 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./tests-1sec76c.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ../references/crates-io-chrono.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/src/logx.rs }
  - { kind: imports, to: ../references/crates-io-clap.md, confidence: extracted, weight: 4, source: crates/inferd-daemon/src/config.rs }
  - { kind: imports, to: ../references/crates-io-inferd-daemon.md, confidence: extracted, weight: 32, source: crates/inferd-daemon/src/main.rs }
  - { kind: imports, to: ../references/crates-io-inferd-engine.md, confidence: extracted, weight: 18, source: crates/inferd-daemon/src/lifecycle.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 31, source: crates/inferd-daemon/src/lifecycle.rs }
  - { kind: imports, to: ../references/crates-io-nix.md, confidence: extracted, weight: 4, source: crates/inferd-daemon/src/peercred.rs }
  - { kind: imports, to: ../references/crates-io-regex.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/src/redact.rs }
  - { kind: imports, to: ../references/crates-io-serde.md, confidence: extracted, weight: 6, source: crates/inferd-daemon/src/config_file.rs }
  - { kind: imports, to: ../references/crates-io-serde-json.md, confidence: extracted, weight: 3, source: crates/inferd-daemon/src/admin.rs }
  - { kind: imports, to: ../references/crates-io-sha2.md, confidence: extracted, weight: 2, source: crates/inferd-daemon/src/fetch.rs }
  - { kind: imports, to: ../references/crates-io-subtle.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/src/fetch.rs }
  - { kind: imports, to: ../references/crates-io-tempfile.md, confidence: extracted, weight: 6, source: crates/inferd-daemon/src/endpoint.rs }
  - { kind: imports, to: ../references/crates-io-tokio.md, confidence: extracted, weight: 39, source: crates/inferd-daemon/src/admin.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 1, source: crates/inferd-daemon/src/lifecycle_v2.rs }
  - { kind: imports, to: ../references/crates-io-tracing.md, confidence: extracted, weight: 19, source: crates/inferd-daemon/src/admin.rs }
  - { kind: imports, to: ../references/crates-io-tracing-subscriber.md, confidence: extracted, weight: 6, source: crates/inferd-daemon/src/logx.rs }
  - { kind: imports, to: ../references/crates-io-windows-sys.md, confidence: extracted, weight: 33, source: crates/inferd-daemon/src/main.rs }
---
# crates/inferd-daemon/src

<!-- signpost:managed:summary -->
21 rust files; 154 exported symbols; entrypoint main; package crates::inferd-daemon::src::admin.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
21 files:
- `crates/inferd-daemon/src/admin.rs`
- `crates/inferd-daemon/src/autoselect.rs`
- `crates/inferd-daemon/src/config.rs`
- `crates/inferd-daemon/src/config_file.rs`
- `crates/inferd-daemon/src/endpoint.rs`
- `crates/inferd-daemon/src/fetch.rs`
- `crates/inferd-daemon/src/lib.rs`
- `crates/inferd-daemon/src/lifecycle.rs`
- `crates/inferd-daemon/src/lifecycle_embed.rs`
- `crates/inferd-daemon/src/lifecycle_rerank.rs`
- `crates/inferd-daemon/src/lifecycle_v2.rs`
- `crates/inferd-daemon/src/lock.rs`
- `crates/inferd-daemon/src/logx.rs`
- `crates/inferd-daemon/src/main.rs`
- `crates/inferd-daemon/src/peercred.rs`
- `crates/inferd-daemon/src/queue.rs`
- `crates/inferd-daemon/src/redact.rs`
- `crates/inferd-daemon/src/router.rs`
- `crates/inferd-daemon/src/status.rs`
- `crates/inferd-daemon/src/store.rs`
- `crates/inferd-daemon/src/windows_security.rs`

- **Exports** (154): `ADMIN_BROADCAST_CAPACITY`, `AcceptContext`, `Admission`, `Admission.available_permits`, `Admission.capacity`, `Admission.new`, `Admission.try_admit`, `AutoselectOutcome`, `BUILD_PROFILE`, `BackendEntry`, `BackendEntry.name`, `BackendKind`, `BedrockInvokeEntry`, `BreakerPolicy`, `Cli`, `Cli.require_one_transport`, `ConfigError`, `ConfigFile`, `ConfigFile.load`, `ConfigFile.resolved_backends`, `Connection`, `Connection.transport`, `DEFAULT_ADMIN_PIPE_PATH`, `DEFAULT_COOLDOWN`, `DEFAULT_FAILURE_THRESHOLD`, `DEFAULT_FAILURE_WINDOW`, `DEFAULT_PIPE_EMBED_PATH`, `DEFAULT_PIPE_PATH`, `DEFAULT_PIPE_RERANK_PATH`, `DEFAULT_ROTATE_BYTES`, `DEFAULT_WRITE_TIMEOUT_SECS`, `Dispatch`, `FetchError`, `ImportError`, `InvalidModelName`, `KEEP_GENERATIONS`, `LONG_VERSION`, `ListenConfig`, `LlamacppEntry`, `LoadPhase`, `Lock`, `Lock.acquire`, `Lock.path`, `LockError`, `LogxLayer`, `LogxLayer.new`, `LogxWriter`, `LogxWriter.open`, `LogxWriter.rotate_now`, `LogxWriter.write_record`, `Manifest`, `ManifestSource`, `ModelAutoselect`, `ModelConfig`, `ModelSpec`, `ModelStore`, `ModelStore.at_platform_default`, `ModelStore.blob_path`, `ModelStore.ensure_layout`, `ModelStore.lock_path`, and 94 more

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×3, [crates/inferd-engine/src/llamacpp/chat_template](./chat-template.md) ×3, [crates/inferd-daemon/tests/common](./common.md) ×2, [clients/go](./go.md) ×8, [crates/inferd-daemon](./inferd-daemon.md) ×20, [crates/inferd-engine](./inferd-engine.md) ×7, [packaging/launchd](./launchd.md) ×4, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×12, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×3, [packaging](./packaging.md) ×3, [crates/inferd-client/src](./src-162lily.md) ×13, [crates/inferd-engine/src](./src-1b7d94r.md) ×11, [crates/inferd/src](./src-1kxpou4.md) ×12, [crates/inferd-proto/src](./src-ymgvev.md) ×3, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×17, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×7, [crates/inferd-proto/tests](./tests-1sec76c.md) ×3, [crates/inferd-proto/src/v2](./v2.md) ×3

- **Imports**: [chrono](../references/crates-io-chrono.md) ×1, [clap](../references/crates-io-clap.md) ×4, [inferd-daemon](../references/crates-io-inferd-daemon.md) ×32, [inferd-engine](../references/crates-io-inferd-engine.md) ×18, [inferd-proto](../references/crates-io-inferd-proto.md) ×31, [nix](../references/crates-io-nix.md) ×4, [regex](../references/crates-io-regex.md) ×1, [serde](../references/crates-io-serde.md) ×6, [serde_json](../references/crates-io-serde-json.md) ×3, [sha2](../references/crates-io-sha2.md) ×2, [subtle](../references/crates-io-subtle.md) ×1, [tempfile](../references/crates-io-tempfile.md) ×6, [tokio](../references/crates-io-tokio.md) ×39, [tokio-stream](../references/crates-io-tokio-stream.md) ×1, [tracing](../references/crates-io-tracing.md) ×19, [tracing-subscriber](../references/crates-io-tracing-subscriber.md) ×6, [windows-sys](../references/crates-io-windows-sys.md) ×33
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
