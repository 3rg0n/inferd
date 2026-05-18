# Contributing to inferd

## Status

**Alpha.** Code is in flight; v0.1 is shipping toward GA. The crate
layout is in `docs/plan-v0.1.md`; the wire protocol is in
`docs/protocol-v1.md`. Contributions against the design are welcome
via issue; code contributions are welcome via PR.

## Development workflow

- Rust toolchain: stable, version tracked in `rust-toolchain.toml`.
- One `cargo workspace` with the crate layout in `docs/plan-v0.1.md`.
- Format: `cargo fmt --all`.
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`.
- Audit: `cargo audit` (third-party CVE DB).
- Deny: `cargo deny check` (license allow-list, duplicate-crate
  detection, banned crates).
- Tests: `cargo test --all`; integration tests that need a real
  llama.cpp build are gated behind the `llamacpp-integration` feature.

Every PR must pass fmt + clippy + test + audit + deny before merge.

## Protocol changes

Changes to `docs/protocol-v1.md` require an ADR documenting the
rationale and a migration note. v1 is frozen by ADR 0008; any
incompatible change goes to v2 on a separate socket path.

## Security

- `THREAT_MODEL.md` is the authoritative posture document. Every PR
  that touches identity enforcement, file permissions, subprocess
  spawning, or the network surface is reviewed against it.
- Security disclosures: security@inferd.io (MX pending). Until that
  mailbox is live, open a private advisory via GitHub's "Security"
  tab on the repo.
