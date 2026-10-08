---
type: Pipeline
title: CI cargo deny (licences + MPL containment)
description: "CI job cargo deny (licences + MPL containment) in the CI workflow, 3 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: cargo deny (licences + MPL containment) }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: actions/checkout → Install cargo-deny → cargo deny check licenses bans sources }
  - { name: steps, value: "3" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI cargo deny (licences + MPL containment)

<!-- signpost:managed:summary -->
CI job cargo deny (licences + MPL containment) in the CI workflow, 3 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
