# Architecture Decision Records

Cross-cutting architectural decisions are recorded here. Small
implementation choices aren't.

| # | Title | Status |
|---|---|---|
| [0001](0001-wire-protocol-inherited-from-thlibo.md) | Wire protocol v1 inherited byte-for-byte from thlibo | Accepted |
| [0002](0002-rust-not-go.md) | Rust, not Go | Accepted |
| [0003](0003-subprocess-llamafile-not-ffi.md) | Subprocess llamafile, not FFI | Accepted |
| [0004](0004-mit-not-apache.md) | MIT license, not Apache-2.0 | Accepted |

## Writing a new ADR

- One page. Nygard short form.
- Filename `NNNN-kebab-case-title.md`, zero-padded sequential.
- Once accepted, ADRs are immutable. To revise, write a new ADR
  and set the old one's status to `superseded by NNNN`.
- Reference the ADR from the corresponding `CHANGELOG.md` entry.
