---
type: Document
title: "ADR 0017: embeddings on a third socket"
description: "Architecture decision (Accepted), 73 rules read from 0017-embeddings-on-a-third-socket.md."
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/docs/adr/0017-embeddings-on-a-third-socket.md
tags: [accepted, adr, constraint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: number, value: "0017" }
  - { name: rules, value: "73" }
  - { name: sections, value: "0017. Embeddings on a third socket — NDJSON, not HTTP, 0017. Embeddings on a third socket — NDJSON, not HTTP / Alternatives considered, 0017. Embeddings on a third socket — NDJSON, not HTTP / Consequences, 0017. Embeddings on a third socket — NDJSON, not HTTP / Context, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Embedding capabilities frame (admin), 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Endpoint: a third dedicated socket, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Frame cap, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Multi-model is still many processes, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Transport: NDJSON-over-IPC, not HTTP, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Wire format / Embed error, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Wire format / Embed request, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / Wire format / Embed response, 0017. Embeddings on a third socket — NDJSON, not HTTP / Decision / v0.2.0 scope: llamacpp only, 0017. Embeddings on a third socket — NDJSON, not HTTP / References" }
  - { name: status, value: Accepted }
---
# ADR 0017: embeddings on a third socket

<!-- signpost:managed:summary -->
Architecture decision (Accepted), 73 rules read from 0017-embeddings-on-a-third-socket.md.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `docs/adr/0017-embeddings-on-a-third-socket.md`
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
