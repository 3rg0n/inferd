---
type: Pipeline
title: CI dl-backends
description: "CI job ${{ matrix.os }} / dl-backends feature in the CI workflow, 8 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@ac3e2963262f5966c036d82e20d35726221e9d07/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: "${{ matrix.os }} / dl-backends feature" }
  - { name: runner, value: "${{ matrix.os }}" }
  - { name: runs, value: actions/checkout → Install Rust → Install libclang (Ubuntu) → Install LLVM (macOS) → Swatinem/rust-cache → clippy → test → Verify backends/ staging populated }
  - { name: steps, value: "8" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-swatinem-rust-cache.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI dl-backends

<!-- signpost:managed:summary -->
CI job ${{ matrix.os }} / dl-backends feature in the CI workflow, 8 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [Swatinem/rust-cache](../references/github-actions-swatinem-rust-cache.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
