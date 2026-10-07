---
type: Module
title: packaging
description: "1 shell file; entrypoint #!."
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/packaging
tags: [entrypoint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "8" }
  - { name: entrypoints, value: "#!" }
  - { name: exported, value: "0" }
  - { name: files, value: "1" }
  - { name: first_commit, value: "2026-05-17" }
  - { name: last_commit, value: "2026-08-10" }
  - { name: lines_added, value: "191" }
  - { name: lines_removed, value: "21" }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./go.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./launchd.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 3 }
  - { kind: co_changes, to: ./windows.md, confidence: extracted, weight: 5 }
---
# packaging

<!-- signpost:managed:summary -->
1 shell file; entrypoint #!.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `packaging/stage-release.sh`

- **Changes with**: [clients/go](./go.md) ×2, [packaging/launchd](./launchd.md) ×6, [crates/inferd-daemon/src](./src-12r0iph.md) ×3, [packaging/windows](./windows.md) ×5
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
