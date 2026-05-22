# 0018. CLI binary renamed back to `inferdctl`

- Status: accepted
- Date: 2026-05-22

## Context

In v0.1.10 the CLI binary was renamed `inferdctl` → `inferd` (see
ADR 0014, "the inferd CLI is a reference middleware"). The intent
was the gh / kubectl shape: one tool name, many subcommands —
`inferd status`, `inferd watch`, `inferd pull`, `inferd doctor`,
plus a future default `inferd -p "..."` prompt mode.

When v0.2.0 went to publish on crates.io, the standalone `inferd`
crate name was already taken — squatted by a placeholder package
unrelated to this project. We have no recourse to claim it short
of a crates.io ownership dispute, which is slow, uncertain, and
not worth the architectural compromise of having the CLI crate be
named differently from the user-facing binary.

The crates.io squat is the forcing function, but it usefully
revealed a second issue independent of registry availability: a
binary called `inferd` adjacent to `inferd-daemon` is genuinely
ambiguous. Operators reading shell history, log lines, or service
files cannot tell at a glance which of "the daemon" vs "the CLI"
a reference points to. The `*ctl` suffix (cf. `systemctl`,
`kubectl`, `etcdctl`, `journalctl`) is the standard signal for
"the operator-facing CLI that talks to a long-running service of
similar name." It's the right shape for what this binary actually
is.

## Decision

The CLI binary is renamed back to **`inferdctl`** for v0.2.1.

- `crates/inferd/Cargo.toml`: `[package].name = "inferdctl"`,
  `[[bin]].name = "inferdctl"`. The directory path stays
  `crates/inferd/` to keep churn (and git blame) minimal — the
  filesystem layout is incidental, the published name is what
  matters.
- `crates/inferd/src/main.rs`: clap `#[command(name = "inferdctl",
  ...)]`; doc-comments reference `inferdctl status` /
  `inferdctl watch` / `inferdctl pull` / `inferdctl doctor`.
- Release pipeline (`.github/workflows/release.yml`) builds
  `--bin inferdctl` and stages `inferdctl` / `inferdctl.exe` into
  the release tarballs.
- Cross-references in `INTEGRATING.md`, daemon source comments,
  and engine source comments are updated to spell `inferdctl`.

ADR 0014's invariants (no private daemon API, no internal
subcommand surface, no special-cased socket paths or auth bypass
— the CLI is a peer of every other consumer) are preserved
*verbatim*. This ADR only changes the binary name; the
architectural posture is identical.

## Consequences

### Why this is the right shape

- **The crate publishes.** v0.2.1 unblocks a clean `cargo publish`
  cadence: `inferd-proto` → `inferd-engine` → `inferd-client` →
  `inferd-daemon` → `inferdctl`. No registry dispute required.
- **Operator-disambiguation.** `inferd-daemon` (service) and
  `inferdctl` (CLI) are now visually distinct in shell history,
  systemd unit files, log lines, and shell completions. Anyone
  who has seen `systemctl` / `kubectl` already knows the shape.
- **The `*ctl` suffix is conventional.** Operators know what to
  expect from a `*ctl`: it's a thin client that introspects /
  controls a long-running service. That's exactly what the binary
  does. ADR 0014's reference-middleware framing is consistent
  with the `*ctl` shape: a `*ctl` is a peer client by convention.

### What this costs

- **One-cycle churn for early adopters.** Anyone who scripted
  against `inferd status` in the v0.1.10 → v0.2.0 window must
  rewrite to `inferdctl status`. Mitigated by: (1) the v0.1.10 →
  v0.2.0 window was short (≈2 weeks, alpha-tier), (2) the four
  shipped subcommands all behave identically post-rename, only
  the binary name changes.
- **Documentation churn.** README, INTEGRATING, ADR 0014's prose,
  CHANGELOG narrative, plan-v0.1.md, packaging unit names all
  reference the CLI by name. This rename touches all of them.
- **The `inferd` name on crates.io stays squatted.** We do not
  pursue a dispute. If the squatter ever transfers / vacates the
  name, we revisit; until then, `inferdctl` is the published
  name.

### What this explicitly does not change

- **ADR 0014 is preserved.** Every invariant (no private daemon
  API, library-only implementation, peer-of-every-consumer)
  remains. `inferdctl` is what `inferd` was, with a different
  name.
- **The directory `crates/inferd/` does not move.** Filesystem
  paths stay; only the published crate name and binary name
  change. A future cleanup could rename the directory to match
  but is not required.
- **The wire protocol is untouched** (ADR 0008). This is a
  packaging-layer rename. Daemon ↔ CLI traffic is unchanged.
- **Release tarball layout** stays the same modulo the binary
  basename. Consumers who installed `inferd` from a v0.2.0
  tarball update by removing `inferd` and installing
  `inferdctl`.

## Alternatives considered

- **Pursue the crates.io ownership dispute.** Rejected: slow,
  uncertain outcome, blocks the v0.2.1 release indefinitely, and
  even if successful re-introduces the operator-disambiguation
  problem. The squat is the forcing function but `inferdctl` is
  independently the right shape.
- **Publish under a different scope (e.g. `io.inferd-cli`).**
  Rejected: crates.io has no namespacing, and adopting a
  contrived name to dodge a squat looks like exactly that. Better
  to pick a name that both publishes *and* reads well.
- **Don't publish the CLI to crates.io; ship it only in the
  release tarball.** Rejected: `cargo install inferdctl` is a
  real workflow for users who want the CLI without a tarball
  install (especially WSL / non-systemd Linux). Removing that
  path to dodge the squat would be a UX regression.
- **Rename to `infctl` / `idctl` / something shorter.** Rejected:
  `inferd-daemon` already has the `inferd` prefix; matching it
  with `inferdctl` keeps the family obvious. Brevity at the cost
  of recognisability is the wrong trade for an alpha-tier tool
  whose users are still learning the surface.

## References

- ADR 0014 — superseded by this one (rename only; invariants
  preserved). The architectural framing of the CLI as a reference
  middleware client carries forward unchanged.
- crates.io: <https://crates.io/crates/inferdctl> (post-v0.2.1).
- `crates/inferd/Cargo.toml`, `crates/inferd/src/main.rs`,
  `.github/workflows/release.yml` — implementation.
