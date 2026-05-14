# Contributing to inferd

## Status

**Planning stage.** No Rust code yet. The repo today is a design
document, a protocol spec, and four ADRs. Contributions against the
design are welcome via issue; code contributions become relevant once
milestone M0 is declared done in `docs/plan-v0.1.md`.

## Development workflow (once M1 starts)

- Rust toolchain: stable, version tracked in `rust-toolchain.toml`.
- One `cargo workspace` with the crate layout in `docs/plan-v0.1.md`.
- Format: `cargo fmt --all`.
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`.
- Audit: `cargo audit` (third-party CVE DB).
- Deny: `cargo deny check` (license allow-list, duplicate-crate
  detection, banned crates).
- Tests: `cargo test --all`; integration tests that need a real
  llamafile are gated behind the `llamafile-integration` feature.

Every PR must pass fmt + clippy + test + audit + deny before merge.

## Protocol changes

Changes to `docs/protocol-v1.md` require an ADR documenting the
rationale and a migration note. v1 is frozen byte-for-byte with
thlibo v0.1 by ADR 0001; any incompatible change goes to v2 on a
separate socket path.

## Security

- Same bar as thlibo: `THREAT_MODEL.md` will be produced at M1 when
  there's code to model. Until then, every ADR that touches security
  posture (identity enforcement, file permissions, subprocess
  spawning) is reviewed against thlibo's `THREAT_MODEL.md`.
- Security disclosures: security@inferd.io (MX pending). Until that
  mailbox is live, open a private advisory via GitHub's "Security"
  tab on the repo.

## Related projects

- **thlibo** — `github.com/3rg0n/thlibo` — Claude Code / Codex CLI
  middleware that consumes inferd once v0.2 ships. The motivating
  use case.
