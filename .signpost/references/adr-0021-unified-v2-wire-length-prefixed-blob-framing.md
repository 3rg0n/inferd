---
type: Document
title: "ADR 0021: unified v2 wire length prefixed blob framing"
description: "Architecture decision (Accepted), 46 rules read from 0021-unified-v2-wire-length-prefixed-blob-framing.md."
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md
tags: [accepted, adr, constraint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: number, value: "0021" }
  - { name: rules, value: "46" }
  - { name: sections, value: "0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Acceptance (issue #34), 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Consequences, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Context, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Decision, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Decision / 1. One generation API, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Decision / 2. Length-prefixed, type-tagged framing, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Decision / 3. Media rides as a raw BLOB frame, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Decision / 4. In-band wire version, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / References, 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version / Why not gRPC / protobuf / Cap'n Proto / shared memory on the IPC wire" }
  - { name: status, value: Accepted }
---
# ADR 0021: unified v2 wire length prefixed blob framing

<!-- signpost:managed:summary -->
Architecture decision (Accepted), 46 rules read from 0021-unified-v2-wire-length-prefixed-blob-framing.md.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md`
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
