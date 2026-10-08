---
type: Pipeline
title: Release Sign + publish release
description: "CI job Sign + publish release in the Release workflow, 9 steps"
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7/.github/workflows/release.yml
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: Sign + publish release }
  - { name: needs, value: "build, sbom" }
  - { name: permissions, value: "contents:write, id-token:write" }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: actions/checkout → actions/download-artifact → Flatten artefact tree → Install cosign → Sign artefacts (keyless OIDC) → Verify asset completeness → Extract CHANGELOG section → Create release → +1 more }
  - { name: steps, value: "9" }
  - { name: workflow, value: Release }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-actions-download-artifact.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-sigstore-cosign-installer.md, confidence: extracted, source: .github/workflows/release.yml }
  - { kind: configures, to: ../references/github-actions-softprops-action-gh-release.md, confidence: extracted, source: .github/workflows/release.yml }
---
# Release Sign + publish release

<!-- signpost:managed:summary -->
CI job Sign + publish release in the Release workflow, 9 steps
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/release.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [actions/download-artifact](../references/github-actions-actions-download-artifact.md), [sigstore/cosign-installer](../references/github-actions-sigstore-cosign-installer.md), [softprops/action-gh-release](../references/github-actions-softprops-action-gh-release.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
