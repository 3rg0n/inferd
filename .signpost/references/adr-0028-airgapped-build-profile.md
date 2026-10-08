---
type: Document
title: "ADR 0028: airgapped build profile"
description: "Architecture decision (Accepted), 71 rules read from 0028-airgapped-build-profile.md."
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175/docs/adr/0028-airgapped-build-profile.md
tags: [accepted, adr, constraint]
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
attributes:
  - { name: number, value: "0028" }
  - { name: rules, value: "71" }
  - { name: sections, value: "0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Alternatives considered, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Consequences, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Context, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / Invert the polarity: a default-on `model-fetch` feature, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / Scope, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / The cloud adapters need no work, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / Two artifacts per platform, one build matrix, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / What the feature gates, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / `default = [\"model-fetch\"]` keeps the normal artifact unchanged, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / Decision / `inferdctl import` — the way bytes get in, 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on / References" }
  - { name: status, value: Accepted }
---
# ADR 0028: airgapped build profile

<!-- signpost:managed:summary -->
Architecture decision (Accepted), 71 rules read from 0028-airgapped-build-profile.md.
<!-- /signpost:managed:summary -->

## Structure

<!-- signpost:managed:structure -->
1 file:
- `docs/adr/0028-airgapped-build-profile.md`
<!-- /signpost:managed:structure -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
