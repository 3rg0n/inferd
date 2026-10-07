---
type: Module
title: packaging/windows
description: 3 powershell files; 1 exported symbol; entrypoint param.
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/packaging/windows
tags: [entrypoint]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: commits, value: "14" }
  - { name: entrypoints, value: param }
  - { name: exported, value: "1" }
  - { name: files, value: "3" }
  - { name: first_commit, value: "2026-05-17" }
  - { name: last_commit, value: "2026-08-08" }
  - { name: lines_added, value: "770" }
  - { name: lines_removed, value: "162" }
  - { name: top_author, value: Ergon Copeland }
  - { name: top_author_share, value: 100% }
edges:
  - { kind: co_changes, to: ./launchd.md, confidence: extracted, weight: 7 }
  - { kind: co_changes, to: ./packaging.md, confidence: extracted, weight: 5 }
---
# packaging/windows

<!-- signpost:managed:summary -->
3 powershell files; 1 exported symbol; entrypoint param.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
3 files:
- `packaging/windows/cleanup-legacy-service.ps1`
- `packaging/windows/install.ps1`
- `packaging/windows/uninstall.ps1`

- **Exports** (1): `Test-Elevation`

- **Changes with**: [packaging/launchd](./launchd.md) ×7, [packaging](./packaging.md) ×5
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
