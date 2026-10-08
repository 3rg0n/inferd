---
type: Pipeline
title: Release CycloneDX SBOM
description: "CI job CycloneDX SBOM in the Release workflow, 6 steps"
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/.github/workflows/release.yml
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: job, value: CycloneDX SBOM }
  - { name: permissions, value: "contents:write, id-token:write" }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: actions/checkout → dtolnay/rust-toolchain → Swatinem/rust-cache → Install cargo-cyclonedx → Generate SBOM → actions/upload-artifact }
  - { name: steps, value: "6" }
  - { name: workflow, value: Release }
edges:
  - { kind: precedes, to: ./release-sign-publish-release.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-actions-upload-artifact.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-dtolnay-rust-toolchain.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-swatinem-rust-cache.md, confidence: extracted, source: .github/workflows/release.yml }
---
# Release CycloneDX SBOM

<!-- signpost:managed:summary -->
CI job CycloneDX SBOM in the Release workflow, 6 steps
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/release.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [actions/upload-artifact](../references/github-actions-actions-upload-artifact.md), [dtolnay/rust-toolchain](../references/github-actions-dtolnay-rust-toolchain.md), [Swatinem/rust-cache](../references/github-actions-swatinem-rust-cache.md)

- **Runs before**: [Release Sign + publish release](./release-sign-publish-release.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
