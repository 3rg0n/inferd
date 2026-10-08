---
type: Pipeline
title: CI airgapped artifact links no HTTPS stack
description: "CI job airgapped artifact links no HTTPS stack in the CI workflow, 5 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: airgapped artifact links no HTTPS stack }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: actions/checkout → Install Rust → Swatinem/rust-cache → Assert no HTTPS client stack in the airgapped trees → Assert hyper is server-only (no client half linked) }
  - { name: steps, value: "5" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-swatinem-rust-cache.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI airgapped artifact links no HTTPS stack

<!-- signpost:managed:summary -->
CI job airgapped artifact links no HTTPS stack in the CI workflow, 5 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [Swatinem/rust-cache](../references/github-actions-swatinem-rust-cache.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
