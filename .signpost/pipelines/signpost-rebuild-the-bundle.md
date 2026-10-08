---
type: Pipeline
title: signpost rebuild the bundle
description: "CI job rebuild the bundle in the signpost workflow, 5 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/.github/workflows/signpost.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: rebuild the bundle }
  - { name: permissions, value: contents:write }
  - { name: runner, value: ubuntu-24.04 }
  - { name: runs, value: actions/checkout → Install signpost → Rebuild the bundle → Verify strictly → Commit the bundle if it changed }
  - { name: steps, value: "5" }
  - { name: workflow, value: signpost }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/signpost.yml }
---
# signpost rebuild the bundle

<!-- signpost:managed:summary -->
CI job rebuild the bundle in the signpost workflow, 5 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/signpost.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
