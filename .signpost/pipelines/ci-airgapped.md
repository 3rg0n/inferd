---
type: Pipeline
title: CI airgapped
description: "CI job ${{ matrix.os }} / airgapped (--no-default-features) in the CI workflow, 6 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: job, value: "${{ matrix.os }} / airgapped (--no-default-features)" }
  - { name: runner, value: "${{ matrix.os }}" }
  - { name: runs, value: actions/checkout → Install Rust → Swatinem/rust-cache → cargo clippy -p inferd-daemon -p inferdctl --no-default-features --all-targets -- -D warnings → cargo test -p inferd-daemon --no-default-features → cargo test -p inferdctl --no-default-features }
  - { name: steps, value: "6" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-swatinem-rust-cache.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI airgapped

<!-- signpost:managed:summary -->
CI job ${{ matrix.os }} / airgapped (--no-default-features) in the CI workflow, 6 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [Swatinem/rust-cache](../references/github-actions-swatinem-rust-cache.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
