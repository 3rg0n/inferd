# inferd release runbook

This document describes how inferd cuts a release. It is not the
CI config; it is the contract the release workflow implements,
and the procedure a human follows when something goes wrong.

## What a release ships

Each release tag (`vX.Y.Z`) produces, on the GitHub Release page:

- 4 platform archives (each containing `inferd-daemon`, `inferdctl`, and `inferd-http`):
  - `inferd-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
  - `inferd-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz`
  - `inferd-vX.Y.Z-aarch64-apple-darwin.tar.gz`
  - `inferd-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- One `*.sha256` sidecar per archive (universal "did this download
  corrupt" check; verify with `shasum -a 256 -c <archive>.sha256`).
- One `*.cosign.bundle` per archive (keyless OIDC provenance; verify
  with `cosign verify-blob --bundle <archive>.cosign.bundle <archive>`).
- One CycloneDX SBOM per workspace crate (`*.cdx.json`).
- A release body extracted from the matching `## [X.Y.Z]` section
  of `CHANGELOG.md`.

If any of those are missing, the workflow's
`Verify asset completeness` step fails before publishing — the
release page is not created.

## Cutting a release

The release workflow is triggered exclusively by pushing a `vX.Y.Z`
tag. Do **not** run `gh release create` manually before the tag is
pushed — that creates a release page the workflow then overwrites.
Just push the tag.

### 1. Land all release content on `main`

Every commit going into the release must be on `main` first. Open a
PR from `vX.Y-dev` to `main`, get it merged, and verify CI is green
on `main` before tagging.

### 2. Bump versions and CHANGELOG

In one commit:

- Bump every workspace crate to `X.Y.Z` in `Cargo.toml`.
- Promote `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` in
  `CHANGELOG.md`. Leave a fresh empty `## [Unreleased]` above it.
- Run `cargo fmt --all && cargo clippy --all-targets --all-features
  -- -D warnings && cargo test --all` and commit the result.

Commit message: `release: vX.Y.Z — promote [Unreleased] to [X.Y.Z]`.

### 3. Tag and push

```sh
git tag -s vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

### 4. Watch the workflow

```sh
gh run watch --workflow=release.yml
```

The expected trajectory:

- 4 `build` jobs run in parallel (~5 minutes each on llama.cpp builds).
- 1 `sbom` job runs in parallel (~3 minutes).
- 1 `publish` job runs after all 4 succeed: signs archives, verifies
  asset completeness, extracts CHANGELOG section, creates the release.

If `Verify asset completeness` fails: at least one platform build
silently shipped fewer-than-expected outputs. Check the upstream
build job logs.

### 5. Publish to crates.io

The workflow does **not** push to crates.io — that is a deliberate
manual step so a borked release page doesn't poison the registry.

Order matters; later crates depend on earlier ones:

```sh
cargo publish -p inferd-proto
cargo publish -p inferd-engine
cargo publish -p inferd-client
cargo publish -p inferd-daemon
cargo publish -p inferdctl
```

Wait ~30 seconds between each so the registry index has time to
propagate before the next dependent build runs.

## When something goes wrong

### A platform build failed mid-workflow

Outcome: `publish` is skipped (it `needs:` all builds). The release
page is not created and no assets are uploaded.

To recover **before retagging**:

1. Fix the underlying build break in a follow-up commit on
   `vX.Y-dev`.
2. Open a PR to `main`, land it.
3. Bump to `vX.Y.Z+1` (per ADR convention; never re-tag a published
   version) and follow the normal procedure.

If the existing tag is salvageable (unlikely but possible): you can
manually attach the workflow's per-platform artifacts to the release
page using `gh release upload`. The artifacts persist for 7 days
under "Actions → Artifacts" on the failed run. Note that re-running
the workflow against an already-existing tag will not re-trigger the
`on: push: tags` event.

### Workflow ran but no assets are attached

Caused by `publish` being skipped (see above) or by a permission
issue with the GitHub token. Check: **Settings → Actions → General →
Workflow permissions** must be `Read and write permissions`.

### Wrong CHANGELOG section in release body

Caused by the `## [X.Y.Z]` heading not matching the tag exactly. The
extractor is `awk` and matches the literal string `## [X.Y.Z]` (no
fuzzy date matching). Fix: edit the release body manually with `gh
release edit vX.Y.Z --notes-file …`. For the next release, ensure
the heading format is exactly `## [X.Y.Z] - YYYY-MM-DD`.

### Cosign signing failed

Likely cause: the workflow lacks `id-token: write` permission. This
is set globally at the workflow level — check `permissions:` in
`release.yml`. If signing has degraded selectively (e.g. one archive
unsigned), check for retries / network flakes in the cosign step
logs and re-run that job.

## Hardening notes

- All third-party Actions are pinned to commit SHAs (not tags). When
  a SHA is bumped, leave a `# vN.N.N` comment so future readers can
  see the pinned upstream version. Tag re-pointing is a real attack
  surface for release pipelines.
- The matrix is `fail-fast: false` so one platform's break doesn't
  cancel the others; their artifacts remain available for the
  recovery path above.
- Release builds do **not** use rust-cache. Correctness > speed; we
  saw the cache serve mock-only target dirs for `llamacpp` builds in
  v0.1.1 and v0.1.4.
- `--bin inferd-daemon` is built with `--features
  inferd-daemon/llamacpp`; the matching `Verify binary advertises
  --backend llamacpp` step is a direct check that the cargo feature
  was actually applied.

## Reference

- [release.yml](../.github/workflows/release.yml) — the workflow.
- [CHANGELOG.md](../CHANGELOG.md) — release notes source of truth.
- [ADR 0014](adr/0014-inferd-cli-is-a-reference-middleware.md) and
  [ADR 0018](adr/0018-cli-renamed-to-inferdctl.md) — CLI binary
  naming. Current published name is `inferdctl`.
