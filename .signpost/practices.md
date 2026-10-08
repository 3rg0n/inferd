---
type: Practices
title: How work is done here
description: "What this repository declares about building, testing, gating, and ownership — and what it does not."
resource: git://github.com/3rg0n/inferd@5ddd30e1a8710360cbcbe60f204e4d5e175d6175
generated: { by: signpost/v0.2.0, at: "2026-10-08" }
---
# How work is done here

Each line is something this repository states, or something it does not. A missing declaration is not a criticism and there is no score here: it is a fact about what an agent can rely on, and the absences are the ones worth reading, because they are what it would otherwise have to guess.
<!-- signpost:managed:practices -->
### Building

- **Not declared.** No build command is declared. An agent asked to build this repository has to infer how, and its first guess is not reviewable.
  - Looked in Makefile targets, package.json scripts, Cargo aliases, CMake targets, Bazel targets.

### Testing

- **Not declared.** No test command is declared. This is the fact an agent most needs before it offers to add a test, because it decides where the test goes and how it is run.
  - Looked in Makefile targets, package.json scripts, Cargo aliases, CMake targets, Bazel targets.
- 29 test files in the tree.

### What runs against a change

- 14 jobs run on a pull request or on a push to the default branch: `${{ matrix.os }} / Tier 5 security regression`, `${{ matrix.os }} / airgapped (--no-default-features)`, `${{ matrix.os }} / default`, `${{ matrix.os }} / dl-backends feature`, `${{ matrix.os }} / go client`, `${{ matrix.os }} / llamacpp feature`, and 8 others. Which of them is *required* is configured on the repository and is not in the tree.
  - Stated in `.github/workflows/ci.yml` line 26, `.github/workflows/pages.yml` line 22, and `.github/workflows/signpost.yml` line 47.
- 4 further CI jobs run outside that gate — on a schedule, a tag, or manually.
  - Stated in `.github/workflows/release.yml` line 69 and `.github/workflows/validate-arm64.yml` line 54.

### How changes are recorded

- Commit subjects follow Conventional Commits: 353 of 358 read, including 88 fix and 80 feature. A message here states what kind of change it is.
- 54 tags reachable from this commit, the most recent `clients/go/v0.8.0` on 2026-08-12, 24 commits back.

### Dependencies

- The Cargo dependencies are pinned by a lockfile.
  - Stated in `Cargo.lock`.
- The Go manifest declares no dependencies, so there is nothing for a lockfile to pin.
  - Stated in `clients/go/go.mod`.
- **Not declared.** No automated dependency updates are configured, so a published CVE in a dependency is found by whoever happens to look.
  - Looked in `.github/dependabot.yml`, `.github/dependabot.yaml`, `renovate.json`, `renovate.json5`, `.renovaterc`, `.renovaterc.json`, and 2 other places.

### Ownership and policy

- **Not declared.** No CODEOWNERS rules were found, so nothing states who reviews a change to a given path.
  - Looked in `CODEOWNERS`, `.github/CODEOWNERS`, and `docs/CODEOWNERS`.
- The repository states its licence.
  - Stated in `LICENSE`.
- **Not declared.** No security policy was found, so someone who finds a vulnerability has to guess where to send it.
  - Looked in `SECURITY.md`, `.github/SECURITY.md`, and `docs/SECURITY.md`.

### Documentation

- The repository has a README.
  - Stated in `README.md`.
- 67 documentation files in the tree, outside the bundle.

### Observability

- **Not declared.** No observability library is a declared dependency, so a failure in production is diagnosed from whatever the code happens to log.
  - Looked in declared dependencies of every manifest read.

### Instructions for agents

- 98 stated rules for agents working in this repository.
  - Stated in `CLAUDE.md`.
- 31 architecture decision records state why things are the way they are.
  - Stated in `docs/adr/0001-wire-protocol-inherited-from-thlibo.md`, `docs/adr/0002-rust-not-go.md`, `docs/adr/0003-subprocess-llamafile-not-ffi.md`, `docs/adr/0004-mit-not-apache.md`, `docs/adr/0005-libllama-ffi-not-subprocess.md`, `docs/adr/0006-lean-core-ecosystem-extensions.md`, and 25 other files.
<!-- /signpost:managed:practices -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
