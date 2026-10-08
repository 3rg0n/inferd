---
type: Module
title: packaging/launchd
description: "2 shell files; entrypoint #!."
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/packaging/launchd
tags: [entrypoint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "14" }
  - { name: entrypoints, value: "#!" }
  - { name: exported, value: "0" }
  - { name: files, value: "2" }
  - { name: first_commit, value: "2026-05-17" }
  - { name: last_commit, value: "2026-08-08" }
  - { name: lines_added, value: "500" }
  - { name: lines_removed, value: "106" }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./inferd-daemon.md, confidence: extracted, weight: 2 }
  - { kind: co_changes, to: ./packaging.md, confidence: extracted, weight: 6 }
  - { kind: co_changes, to: ./src-12r0iph.md, confidence: extracted, weight: 4 }
  - { kind: co_changes, to: ./windows.md, confidence: extracted, weight: 7 }
---
# packaging/launchd

<!-- signpost:managed:summary -->
2 shell files; entrypoint #!.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
2 files:
- `packaging/launchd/install-launchagent.sh`
- `packaging/launchd/uninstall-launchagent.sh`

- **Changes with**: [crates/inferd-daemon](./inferd-daemon.md) ×2, [packaging](./packaging.md) ×6, [crates/inferd-daemon/src](./src-12r0iph.md) ×4, [packaging/windows](./windows.md) ×7
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
