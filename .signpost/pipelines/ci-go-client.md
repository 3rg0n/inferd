---
type: Pipeline
title: CI go-client
description: "CI job ${{ matrix.os }} / go client in the CI workflow, 7 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: "${{ matrix.os }} / go client" }
  - { name: runner, value: "${{ matrix.os }}" }
  - { name: runs, value: actions/checkout → Install Rust → actions/setup-go → Swatinem/rust-cache → cargo build -p inferd-daemon → go vet → go test }
  - { name: steps, value: "7" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-actions-setup-go.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-swatinem-rust-cache.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI go-client

<!-- signpost:managed:summary -->
CI job ${{ matrix.os }} / go client in the CI workflow, 7 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [actions/setup-go](../references/github-actions-actions-setup-go.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [Swatinem/rust-cache](../references/github-actions-swatinem-rust-cache.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
