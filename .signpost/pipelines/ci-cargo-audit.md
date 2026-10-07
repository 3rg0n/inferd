---
type: Pipeline
title: CI cargo audit
description: "CI job cargo audit in the CI workflow, 3 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: cargo audit }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: actions/checkout → Install cargo-audit → cargo audit }
  - { name: steps, value: "3" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI cargo audit

<!-- signpost:managed:summary -->
CI job cargo audit in the CI workflow, 3 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
