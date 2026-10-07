---
type: Pipeline
title: CI systemd-unit smoke (ubuntu)
description: "CI job systemd-unit smoke (ubuntu) in the CI workflow, 14 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/.github/workflows/ci.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: systemd-unit smoke (ubuntu) }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: "actions/checkout → Install Rust → Swatinem/rust-cache → Build daemon (mock backend, release) → Set up systemd user session → Install daemon binary + systemd unit → Assert a genuinely fresh install (issue → Start inferd via systemctl --user → +6 more" }
  - { name: steps, value: "14" }
  - { name: workflow, value: CI }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/ci.yml }
  - { kind: configures, to: ../references/github-actions-swatinem-rust-cache.md, confidence: extracted, source: .github/workflows/ci.yml }
---
# CI systemd-unit smoke (ubuntu)

<!-- signpost:managed:summary -->
CI job systemd-unit smoke (ubuntu) in the CI workflow, 14 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/ci.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [Swatinem/rust-cache](../references/github-actions-swatinem-rust-cache.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
