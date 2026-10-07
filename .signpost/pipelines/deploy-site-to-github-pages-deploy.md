---
type: Pipeline
title: Deploy site to GitHub Pages deploy
description: "CI job deploy in the Deploy site to GitHub Pages workflow, 4 steps; runs on a pull request or a default-branch push"
resource: git://github.com/3rg0n/inferd@2c1a482515d2be7f5a17a6298602258f5647962d/.github/workflows/pages.yml
tags: [gate]
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
attributes:
  - { name: job, value: deploy }
  - { name: permissions, value: "contents:read, id-token:write, pages:write" }
  - { name: runner, value: ubuntu-latest }
  - { name: runs, value: Checkout → Configure Pages → Upload artifact → Deploy to GitHub Pages }
  - { name: steps, value: "4" }
  - { name: workflow, value: Deploy site to GitHub Pages }
edges:
  - { kind: configures, to: ../references/github-actions-actions-checkout.md, confidence: extracted, source: .github/workflows/pages.yml }
  - { kind: configures, to: ../references/github-actions-actions-configure-pages.md, confidence: extracted, source: .github/workflows/pages.yml }
  - { kind: configures, to: ../references/github-actions-actions-deploy-pages.md, confidence: extracted, source: .github/workflows/pages.yml }
  - { kind: configures, to: ../references/github-actions-actions-upload-pages-artifact.md, confidence: extracted, source: .github/workflows/pages.yml }
---
# Deploy site to GitHub Pages deploy

<!-- signpost:managed:summary -->
CI job deploy in the Deploy site to GitHub Pages workflow, 4 steps; runs on a pull request or a default-branch push
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `.github/workflows/pages.yml`

- **Configures**: [actions/checkout](../references/github-actions-actions-checkout.md), [actions/configure-pages](../references/github-actions-actions-configure-pages.md), [actions/deploy-pages](../references/github-actions-actions-deploy-pages.md), [actions/upload-pages-artifact](../references/github-actions-actions-upload-pages-artifact.md)
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
