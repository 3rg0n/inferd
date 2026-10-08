---
type: Document
title: CLAUDE.md
description: "Stated constraints, 98 rules read from CLAUDE.md."
resource: git://github.com/3rg0n/inferd@5324ed9b7adf36de21d22a1bce59482738837d94/CLAUDE.md
tags: [agent-rules, constraint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: rules, value: "98" }
  - { name: sections, value: "CLAUDE.md, CLAUDE.md / Architecture, CLAUDE.md / Architecture / Flow at runtime, CLAUDE.md / Commands / Building and running with a backend, CLAUDE.md / Commands / Cutting a version bump, CLAUDE.md / Commands / First clone, CLAUDE.md / Commands / Pre-commit gate, CLAUDE.md / Commands / Test tiers (`docs/test-strategy.md`), CLAUDE.md / Model reference material, CLAUDE.md / Model store, CLAUDE.md / Non-negotiable invariants (from `context.md` §\"Invariants\"), CLAUDE.md / Project status, CLAUDE.md / Repository map, CLAUDE.md / Scope gates (what NOT to build), CLAUDE.md / What inferd is, CLAUDE.md / When writing ADRs, CLAUDE.md / Wire protocol is frozen — one generation surface + embeddings + rerank" }
edges:
  - { kind: documents, to: ../modules/bedrock-invoke.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/chat-template.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/common.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/cpp.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/embed.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/fuzz-targets.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/go.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/inferd-daemon.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/inferd-engine.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/launchd.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/llamacpp.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/openai-compat.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/packaging.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/rerank.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-12r0iph.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-162lily.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-1b7d94r.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-1kxpou4.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-1qcto7x.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-fq5wh.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/src-ymgvev.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/tests-1285xfa.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/tests-1hsi7ps.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/tests-1qbzbiv.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/tests-1sec76c.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/v2.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/validate.md, confidence: extracted, source: CLAUDE.md }
  - { kind: documents, to: ../modules/windows.md, confidence: extracted, source: CLAUDE.md }
---
# CLAUDE.md

<!-- signpost:managed:summary -->
Stated constraints, 98 rules read from CLAUDE.md.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `CLAUDE.md`

- **Documents**: [crates/inferd-engine/src/bedrock_invoke](../modules/bedrock-invoke.md), [crates/inferd-engine/src/llamacpp/chat_template](../modules/chat-template.md), [crates/inferd-daemon/tests/common](../modules/common.md), [crates/inferd-engine/cpp](../modules/cpp.md), [crates/inferd-proto/src/embed](../modules/embed.md), [crates/inferd-proto/fuzz/fuzz_targets](../modules/fuzz-targets.md), [clients/go](../modules/go.md), [crates/inferd-daemon](../modules/inferd-daemon.md), [crates/inferd-engine](../modules/inferd-engine.md), [packaging/launchd](../modules/launchd.md), [crates/inferd-engine/src/llamacpp](../modules/llamacpp.md), [crates/inferd-engine/src/openai_compat](../modules/openai-compat.md), [packaging](../modules/packaging.md), [crates/inferd-proto/src/rerank](../modules/rerank.md), [crates/inferd-daemon/src](../modules/src-12r0iph.md), [crates/inferd-client/src](../modules/src-162lily.md), [crates/inferd-engine/src](../modules/src-1b7d94r.md), [crates/inferd/src](../modules/src-1kxpou4.md), [crates/inferd-openai-wire/src](../modules/src-1qcto7x.md), [crates/inferd-http/src](../modules/src-fq5wh.md), [crates/inferd-proto/src](../modules/src-ymgvev.md), [crates/inferd-daemon/tests](../modules/tests-1285xfa.md), [crates/inferd-engine/tests](../modules/tests-1hsi7ps.md), [crates/inferd/tests](../modules/tests-1qbzbiv.md), [crates/inferd-proto/tests](../modules/tests-1sec76c.md), [crates/inferd-proto/src/v2](../modules/v2.md), [packaging/validate](../modules/validate.md), [packaging/windows](../modules/windows.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
