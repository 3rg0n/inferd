---
type: Module
title: clients/go
description: 14 go files; 86 exported symbols; package inferd.
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/clients/go
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "25" }
  - { name: exported, value: "86" }
  - { name: files, value: "14" }
  - { name: first_commit, value: "2026-05-14" }
  - { name: last_commit, value: "2026-08-12" }
  - { name: lines_added, value: "3117" }
  - { name: lines_removed, value: "642" }
  - { name: package, value: inferd }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 96% }
edges:
  - { kind: co_changes, to: ./bedrock-invoke.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./chat-template.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./inferd-engine.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./llamacpp.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./openai-compat.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./packaging.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./src-162lily.md, confidence: extracted, weight: 8 }
  - { kind: co_changes, to: ./src-1b7d94r.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./src-fq5wh.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./tests-1285xfa.md, confidence: extracted, weight: 5 }
  - { kind: co_changes, to: ./tests-1hsi7ps.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./v2.md, confidence: extracted, weight: 4 }
---
# clients/go

<!-- signpost:managed:summary -->
14 go files; 86 exported symbols; package inferd.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
14 files:
- `clients/go/addr.go`
- `clients/go/admin.go`
- `clients/go/admin_test.go`
- `clients/go/busy_windows_test.go`
- `clients/go/client.go`
- `clients/go/client_test.go`
- `clients/go/client_unix.go`
- `clients/go/client_v2.go`
- `clients/go/client_v2_test.go`
- `clients/go/client_windows.go`
- `clients/go/dial_test_unix_test.go`
- `clients/go/dial_test_windows_test.go`
- `clients/go/protocol.go`
- `clients/go/protocol_v2.go`

- **Exports** (86): `AdminClient`, `AdminClient.Close`, `AdminClient.Recv`, `AdminClient.WaitReady`, `AdminEvent`, `AdminEvent.IsCapabilities`, `AdminEvent.RequiredAudioSampleRate`, `AdminEvent.SupportsAudio`, `AdminEvent.SupportsRerank`, `AdminEvent.SupportsVision`, `AttachmentAudio`, `AttachmentImage`, `AttachmentKind`, `AttachmentV2`, `AttachmentVideo`, `AudioAttachment`, `AudioBlock`, `BlobDescriptor`, `BlockText`, `BlockThinking`, `BlockToolUse`, `BlockType`, `Client`, `Client.Close`, `Client.GenerateV2`, `ContentAudio`, `ContentBlock`, `ContentImage`, `ContentText`, `ContentToolResult`, `ContentToolUse`, `ContentType`, `ContentVideo`, `DefaultAdminAddr`, `DefaultInferAddr`, `DefaultInferEmbedAddr`, `DefaultInferRerankAddr`, `DefaultInferV2Addr`, `DialAdmin`, `DialAndWaitReady`, `DialInfer`, `DialPipe`, `DialUDS`, `ErrV2AttachmentUnsupported`, `ErrV2BackendUnavailable`, `ErrV2FrameTooLarge`, `ErrV2Internal`, `ErrV2InvalidRequest`, `ErrV2QueueFull`, `ErrV2ToolCallMalformed`, `ErrV2WireVersionUnsupported`, `ErrorCodeV2`, `ImageAttachment`, `ImageBlock`, `JSONSchemaFormat`, `MaxFrameBytes`, `MessageV2`, `New`, `RequestV2`, `ResponseBlockV2`, and 26 more

- **Changes with**: [crates/inferd-engine/src/bedrock_invoke](./bedrock-invoke.md) ×2, [crates/inferd-engine/src/llamacpp/chat_template](./chat-template.md) ×2, [crates/inferd-daemon](./inferd-daemon.md) ×5, [crates/inferd-engine](./inferd-engine.md) ×8, [crates/inferd-engine/src/llamacpp](./llamacpp.md) ×3, [crates/inferd-engine/src/openai_compat](./openai-compat.md) ×2, [packaging](./packaging.md) ×2, [crates/inferd-daemon/src](./src-12r0iph.md) ×8, [crates/inferd-client/src](./src-162lily.md) ×8, [crates/inferd-engine/src](./src-1b7d94r.md) ×2, [crates/inferd-http/src](./src-fq5wh.md) ×2, [crates/inferd-daemon/tests](./tests-1285xfa.md) ×5, [crates/inferd-engine/tests](./tests-1hsi7ps.md) ×4, [crates/inferd-proto/src/v2](./v2.md) ×4
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
