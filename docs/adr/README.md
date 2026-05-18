# Architecture Decision Records

Cross-cutting architectural decisions are recorded here. Small
implementation choices aren't.

| # | Title | Status |
|---|---|---|
| [0001](0001-wire-protocol-inherited-from-thlibo.md) | Wire protocol v1 inherited byte-for-byte from thlibo | Superseded by 0008 |
| [0002](0002-rust-not-go.md) | Rust, not Go | Accepted |
| [0003](0003-subprocess-llamafile-not-ffi.md) | Subprocess llamafile, not FFI | Superseded by 0005 |
| [0004](0004-mit-not-apache.md) | MIT license, not Apache-2.0 | Accepted |
| [0005](0005-libllama-ffi-not-subprocess.md) | Consume libllama via FFI, not llamafile as a subprocess | Accepted |
| [0006](0006-lean-core-ecosystem-extensions.md) | Lean core, ecosystem extensions live as separate processes | Accepted |
| [0007](0007-backend-routing-and-failure-semantics.md) | Backend routing: operator policy, no in-daemon retry, no mid-stream failover | Accepted |
| [0008](0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md) | Protocol v1 designed for inferd, not derived from thlibo | Accepted |
| [0009](0009-pre-m1-open-questions-resolved.md) | Pre-M1 open questions resolved (admin socket, peer creds, versioning, backend identity) | Accepted |
| [0010](0010-narrow-https-exception-for-model-bootstrap.md) | Narrow HTTPS exception for first-boot model bootstrap | Accepted |

## Writing a new ADR

- One page. Nygard short form.
- Filename `NNNN-kebab-case-title.md`, zero-padded sequential.
- Once accepted, ADRs are immutable. To revise, write a new ADR
  and set the old one's status to `superseded by NNNN`.
- Reference the ADR from the corresponding `CHANGELOG.md` entry.
