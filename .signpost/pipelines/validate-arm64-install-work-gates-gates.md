---
type: Pipeline
title: Validate arm64 (install=work gates) gates
description: "CI job ${{ matrix.name }} in the Validate arm64 (install=work gates) workflow, 21 steps"
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/.github/workflows/validate-arm64.yml
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: "${{ matrix.name }}" }
  - { name: permissions, value: contents:read }
  - { name: runner, value: "${{ matrix.os }}" }
  - { name: runs, value: Skip unselected target → actions/checkout → Install Rust → Report free disk → Install libclang (Linux) → Install LLVM (Windows arm64) → Set up MSVC dev environment (Windows arm64) → Build inferd-daemon → +13 more }
  - { name: steps, value: "21" }
  - { name: workflow, value: Validate arm64 (install=work gates) }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/validate-arm64.yml }
  - { kind: configures, to: ../references/github-actions-actions-upload-artifact.md, confidence: extracted, source: .github/workflows/validate-arm64.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/validate-arm64.yml }
  - { kind: configures, to: ../references/github-actions-ilammy-msvc-dev-cmd.md, confidence: extracted, source: .github/workflows/validate-arm64.yml }
  - { kind: configures, to: ../references/github-actions-kylemayes-install-llvm-action.md, confidence: extracted, source: .github/workflows/validate-arm64.yml }
---
# Validate arm64 (install=work gates) gates

<!-- signpost:managed:summary -->
CI job ${{ matrix.name }} in the Validate arm64 (install=work gates) workflow, 21 steps
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/validate-arm64.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [actions/upload-artifact](../references/github-actions-actions-upload-artifact.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [ilammy/msvc-dev-cmd](../references/github-actions-ilammy-msvc-dev-cmd.md), [KyleMayes/install-llvm-action](../references/github-actions-kylemayes-install-llvm-action.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
