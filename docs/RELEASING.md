# inferd release runbook

This document describes how inferd cuts a release. It is not the
CI config; it is the contract the release workflow implements,
and the procedure a human follows when something goes wrong.

## What a release ships

Each release tag (`vX.Y.Z`) produces, on the GitHub Release page:

- 10 platform archives — **two per platform** (each containing
  `inferd-daemon`, `inferdctl`, and `inferd-http`) across 5 targets:
  - `inferd-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
  - `inferd-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz`
  - `inferd-vX.Y.Z-aarch64-apple-darwin.tar.gz`
  - `inferd-vX.Y.Z-x86_64-pc-windows-msvc.zip`
  - `inferd-vX.Y.Z-aarch64-pc-windows-msvc.zip`
  - …and an `inferd-airgapped-vX.Y.Z-<target>` counterpart for each.

  The airgapped set is the same commit built `--no-default-features`
  ([ADR 0028](adr/0028-airgapped-build-profile.md)): the default-on
  `model-fetch` feature is off, so `ureq`/`rustls`/`ring` are not
  linked and models must arrive via `inferdctl import`. Both archives
  contain identical `backends/`, docs, and packaging scripts — the
  binaries are the only difference, and they say which they are
  (`--version` prints `build profile: networked|airgapped`). The
  `no-network-deps` CI job asserts the dependency absence on every PR;
  the release job re-verifies it on the built binary before packing.
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

- Bump every workspace crate to `X.Y.Z` in `Cargo.toml`. Remember the
  ~10 internal `version = "="` path-dep pins do **not** inherit the
  workspace version: `grep -rn 'version = "=' --include=Cargo.toml
  crates/`, then `cargo update -w` for the lockfile.
- Promote `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` in
  `CHANGELOG.md`. Leave a fresh empty `## [Unreleased]` above it.
- **Update the user-facing version strings that are not derived from
  `Cargo.toml`** — nothing generates these, so they drift silently and
  a reader copy-pastes a 404:
  - `README.md` — the `**Status: vX.Y.Z.**` line *and* the tarball /
    zip filenames in the three Install snippets (Linux, macOS,
    Windows). These went stale for a full minor cycle because this
    step didn't exist.
  - `site/index.html` — the masthead version, the "Download" button
    label, the three install snippets, and the colophon status.
  - Then verify: `grep -rn "vX\.Y\.Z-1" README.md site/index.html`
    should return nothing (substitute the *previous* version).
- Run `cargo fmt --all && cargo clippy --all-targets --all-features
  -- -D warnings && cargo test --all` and commit the result.

Commit message: `release: vX.Y.Z — promote [Unreleased] to [X.Y.Z]`.

### 3. Tag and push

```sh
git tag -a vX.Y.Z -F -   # annotated; summarise the release in the message
git push origin vX.Y.Z
```

`-a`, not `-s`. This line said `-s` from v0.1 through v0.8.0 and was
never once followed: `git tag -v` on every shipped tag (v0.6.0 through
v0.8.0) reports `error: no signature found`, because the maintainer's box
has no signing key configured. Release provenance comes from the
workflow's **keyless cosign bundles** — one per archive, verifiable with
`cosign verify-blob --bundle` — which is the mechanism actually in use
and does not depend on a local key. If GPG-signed tags are wanted later,
that is a real change (key, distribution, a documented verification
step), not a flag swap: make it deliberately rather than leaving an
instruction here that the next release will also ignore.

### 4. Watch the workflow

```sh
gh run watch --workflow=release.yml
```

**Budget 2–3 hours wall-clock, not minutes.** Measured on the last two
releases (v0.7.0 / v0.8.0):

| Job | v0.7.0 | v0.8.0 |
|---|---|---|
| `aarch64-unknown-linux-gnu` | 6m | 6m |
| `aarch64-apple-darwin` | 8m | 8m |
| `aarch64-pc-windows-msvc` | 9m | 9m |
| `x86_64-unknown-linux-gnu` (CUDA) | 124m | 127m |
| `x86_64-pc-windows-msvc` (CUDA) | 172m | 128m |
| `sbom` | <1m | 1m |
| `publish` | 1m | 3m |

The two x86_64 legs are **~15× slower than the arm64 ones**, and the
differentiator is CUDA, not architecture: those are the only targets
whose `features:` include `inferd-daemon/cuda`, so `nvcc` compiles device
code for every supported GPU architecture — and does it twice, once per
archive (see below). The arm64 legs build no CUDA at all.

This entry previously read "~5 minutes each," which is wrong by a factor
of ~25 and is worth correcting because the failure mode is a human one:
a run that is *normally* two hours in looks indistinguishable from a
hung one against a five-minute expectation, and the temptation is to
cancel and re-tag a release that was fine. If a CUDA leg is still going
at 90 minutes, that is the middle of its range — check which step it is
on (`gh api repos/3rg0n/inferd/actions/jobs/<id> --jq '.steps[]'`)
before concluding anything. No job sets `timeout-minutes`, so all
inherit GitHub's 360m default; even the 172m worst case has ~3h of head
room.

Order of events:

- 5 `build` jobs run in parallel. Each builds twice and packs two
  archives (ADR 0028). On the CUDA targets the second pass is **not** the
  cheap relink the `--no-default-features` change might suggest: measured
  per-step on v0.8.0's `x86_64-unknown-linux-gnu`, `Build daemon (with
  dl-backends)` took **61m** and `Build daemon (airgapped)` took **60m**.
  Changing the feature set invalidates the cache, so the whole ggml/CUDA
  tree recompiles. Everything either side of those two steps is ≤1m —
  staging, verification and packing are free; essentially the entire leg
  is those two compiles.
- 1 `sbom` job runs in parallel (~1 minute).
- 1 `publish` job runs after all 6 succeed: signs archives, verifies
  asset completeness, extracts CHANGELOG section, creates the release.

If `Verify asset completeness` fails: at least one platform build
silently shipped fewer-than-expected outputs. Check the upstream
build job logs. That step counts the networked and airgapped archives
**separately** — a total of 10 that happened to be 10 networked ones
would otherwise pass while half the release was missing.

### 5. Publish to crates.io (manual, by design)

The workflow does **not** push to crates.io. That is deliberate: a
crates.io version is permanent, so publishing stays a human decision
taken *after* the release page looks right, and the token stays off the
repo's secret list.

**This step is easy to forget, and nothing will remind you** — that is
the cost of doing it by hand. The binaries on the release page and the
crates on the registry are separate deliverables; a green release run
says nothing about the latter.

There was a `crates-io` job here through v0.6.1. It was removed because
it could not fail safely — with no `CARGO_REGISTRY_TOKEN` secret it
no-op'd with a `::notice::` and reported **success**, so a green release
run looked like the crates had shipped when they hadn't. That is exactly
what happened at v0.6.1 (published by hand the same day). A job that
silently does nothing is worse than no job.

Only **two** crates are published: `inferd-proto` and `inferd-client`.
The daemon, engine, and `inferdctl` ship as binaries on the GitHub
release — they are not registry crates.

**First**, prove the tree is identical to the tag for both crate
directories. The registry keeps whatever you send, forever, so publishing
from a tree that has drifted past the tag ships something no tag
describes:

```sh
git diff --stat "v${VER}" -- crates/inferd-proto crates/inferd-client   # MUST be empty
```

Then publish, in dependency order — `inferd-client` depends on
`inferd-proto`:

```sh
cargo publish -p inferd-proto --dry-run   # packages + compiles, aborts before upload
cargo publish -p inferd-proto
# wait ~30s for the index to pick proto up, then:
cargo publish -p inferd-client
```

The wait matters: the `inferd-client` publish resolves `inferd-proto` from
the registry, not the workspace, and fails if it isn't indexed yet. The
token lives in `CARGO_HOME/credentials.toml`, so no env var is needed.

Finally confirm against the registry rather than trusting the CLI's own
`Uploading`/`Published` lines:

```sh
for c in inferd-proto inferd-client; do
  printf '%s: ' "$c"
  curl -s -H "User-Agent: inferd-release-check" \
    "https://crates.io/api/v1/crates/$c" \
    | python -c "import json,sys; print(json.load(sys.stdin)['crate']['max_version'])"
done
```

The `User-Agent` is **required** — crates.io's data-access policy rejects
UA-less requests with an `errors` body, not a 4xx, so a naive one-liner
looks like a parse bug rather than a refusal. On the maintainer's corp
network add `--ssl-no-revoke` (Cisco Secure Access breaks OCSP revocation
checks; never disable cert validation itself). Corp TLS inspection does
**not** block the crates.io upload API — the v0.6.1 crates were published
straight through it, so there's no need to route publishing via CI.

The token lives in `CARGO_HOME/credentials.toml` (on the maintainer's
box `CARGO_HOME` is **not** `~/.cargo`), so `cargo publish` needs no
environment variable. Corp TLS inspection does not block the upload API
— the v0.6.1 crates were published through it.

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
