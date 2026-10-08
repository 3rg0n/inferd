---
type: Pipeline
title: Release build
description: "CI job ${{ matrix.target }} in the Release workflow, 31 steps"
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/.github/workflows/release.yml
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: "${{ matrix.target }}" }
  - { name: permissions, value: "contents:write, id-token:write" }
  - { name: runner, value: "${{ matrix.os }}" }
  - { name: runs, value: "actions/checkout → Install Rust → Install libclang (Ubuntu) → Install LLVM (macOS) → Install LLVM (Windows arm64) → Set up MSVC dev environment (Windows arm64) → Install CUDA toolkit (Linux x86_64 only) → Install cuBLAS (Linux x86_64 only, post-Jimver) → +23 more" }
  - { name: steps, value: "31" }
  - { name: workflow, value: Release }
edges:
  - { kind: precedes, to: ./release-sign-publish-release.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-actions-upload-artifact.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-ilammy-msvc-dev-cmd.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-jimver-cuda-toolkit.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-kylemayes-install-llvm-action.md, confidence: extracted, source: .github/workflows/release.yml }
---
# Release build

<!-- signpost:managed:summary -->
CI job ${{ matrix.target }} in the Release workflow, 31 steps
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/release.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [actions/upload-artifact](../references/github-actions-actions-upload-artifact.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [ilammy/msvc-dev-cmd](../references/github-actions-ilammy-msvc-dev-cmd.md), [Jimver/cuda-toolkit](../references/github-actions-jimver-cuda-toolkit.md), [KyleMayes/install-llvm-action](../references/github-actions-kylemayes-install-llvm-action.md)

- **Runs before**: [Release Sign + publish release](./release-sign-publish-release.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
