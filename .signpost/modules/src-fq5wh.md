---
type: Module
title: crates/inferd-http/src
description: 7 rust files; 33 exported symbols; entrypoint main; package crates::inferd-http::src::audio_decode.
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/crates/inferd-http/src
tags: [entrypoint]
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: commits, value: "10" }
  - { name: entrypoints, value: main }
  - { name: exported, value: "33" }
  - { name: files, value: "7" }
  - { name: first_commit, value: "2026-07-10" }
  - { name: last_commit, value: "2026-10-07" }
  - { name: lines_added, value: "3392" }
  - { name: lines_removed, value: "114" }
  - { name: package, value: crates::inferd-http::src::audio_decode }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-1qcto7x.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 3 }
  - { kind: imports, to: ../references/crates-io-anyhow.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/main.rs }
  - { kind: imports, to: ../references/crates-io-axum.md, confidence: extracted, weight: 15, source: crates/inferd-http/src/error.rs }
  - { kind: imports, to: ../references/crates-io-base64.md, confidence: extracted, weight: 3, source: crates/inferd-http/src/audio_decode.rs }
  - { kind: imports, to: ../references/crates-io-clap.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/config.rs }
  - { kind: imports, to: ../references/crates-io-futures-util.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/handlers.rs }
  - { kind: imports, to: ../references/crates-io-image.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/image_decode.rs }
  - { kind: imports, to: ../references/crates-io-inferd-client.md, confidence: extracted, weight: 3, source: crates/inferd-http/src/handlers.rs }
  - { kind: imports, to: ../references/crates-io-inferd-openai-wire.md, confidence: extracted, weight: 32, source: crates/inferd-http/src/error.rs }
  - { kind: imports, to: ../references/crates-io-inferd-proto.md, confidence: extracted, weight: 17, source: crates/inferd-http/src/error.rs }
  - { kind: imports, to: ../references/crates-io-rubato.md, confidence: extracted, weight: 3, source: crates/inferd-http/src/audio_decode.rs }
  - { kind: imports, to: ../references/crates-io-subtle.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/handlers.rs }
  - { kind: imports, to: ../references/crates-io-symphonia.md, confidence: extracted, weight: 6, source: crates/inferd-http/src/audio_decode.rs }
  - { kind: imports, to: ../references/crates-io-thiserror.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/translate.rs }
  - { kind: imports, to: ../references/crates-io-tokio-stream.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/handlers.rs }
  - { kind: imports, to: ../references/crates-io-tracing-subscriber.md, confidence: extracted, weight: 1, source: crates/inferd-http/src/main.rs }
---
# crates/inferd-http/src

<!-- signpost:managed:summary -->
7 rust files; 33 exported symbols; entrypoint main; package crates::inferd-http::src::audio_decode.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
7 files:
- `crates/inferd-http/src/audio_decode.rs`
- `crates/inferd-http/src/config.rs`
- `crates/inferd-http/src/error.rs`
- `crates/inferd-http/src/handlers.rs`
- `crates/inferd-http/src/image_decode.rs`
- `crates/inferd-http/src/main.rs`
- `crates/inferd-http/src/translate.rs`

- **Exports** (33): `AudioDecodeError`, `ChunkBuilder`, `ChunkBuilder.complete`, `ChunkBuilder.finalize`, `ChunkBuilder.ingest`, `ChunkBuilder.new`, `Config`, `Config.parse`, `DecodedAudio`, `DecodedAudio.to_le_bytes`, `DecodedImage`, `EncodingFormat`, `Endpoint`, `HttpError`, `HttpError.bad_request`, `HttpError.daemon_unreachable`, `HttpError.from_inferd`, `HttpError.new`, `ImageDecodeError`, `MAX_AUDIO_CLIPS_PER_REQUEST`, `MAX_DIM`, `MAX_IMAGES_PER_REQUEST`, `MAX_TOTAL_DECODED_ATTACHMENT_BYTES`, `TranslateError`, `chat_request_to_v2`, `decode_encoded_audio`, `decode_encoded_image`, `decode_image_url`, `decode_input_audio`, `embeddings_request_to_inferd`, `embeddings_response_to_openai`, `router`, `stop_reason_to_openai`

- **Changes with**: [clients/go](./go.md) ×2, [crates/inferd-engine](./inferd-engine.md) ×2, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×4, [crates/inferd-client/src](./src-162lily.md) ×2, [crates/inferd-openai-wire/src](./src-1qcto7x.md) ×6, [crates/inferd-proto/src/v2](./v2.md) ×3

- **Imports**: [anyhow](../references/crates-io-anyhow.md) ×1, [axum](../references/crates-io-axum.md) ×15, [base64](../references/crates-io-base64.md) ×3, [clap](../references/crates-io-clap.md) ×1, [futures-util](../references/crates-io-futures-util.md) ×1, [image](../references/crates-io-image.md) ×1, [inferd-client](../references/crates-io-inferd-client.md) ×3, [inferd-openai-wire](../references/crates-io-inferd-openai-wire.md) ×32, [inferd-proto](../references/crates-io-inferd-proto.md) ×17, [rubato](../references/crates-io-rubato.md) ×3, [subtle](../references/crates-io-subtle.md) ×1, [symphonia](../references/crates-io-symphonia.md) ×6, [thiserror](../references/crates-io-thiserror.md) ×1, [tokio-stream](../references/crates-io-tokio-stream.md) ×1, [tracing-subscriber](../references/crates-io-tracing-subscriber.md) ×1
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
