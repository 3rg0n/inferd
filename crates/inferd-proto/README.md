# inferd-proto

Wire format types and NDJSON framing for [inferd](https://github.com/3rg0n/inferd),
a local-inference daemon. This crate is the canonical schema reference
for clients in any language: Rust clients depend on it directly;
Go/Python/TypeScript clients use it as the source of truth.

The authoritative spec lives at
[`docs/protocol-v1.md`](https://github.com/3rg0n/inferd/blob/main/docs/protocol-v1.md)
in the upstream repo. This crate carries the `serde`-derived Rust
types plus a small NDJSON reader/writer with the 64 MiB per-frame
cap (THREAT_MODEL F-5).

## What's in here

- `Request`, `Resolved`, `Message`, `Role`, `ImageTokenBudget`,
  `VALID_IMAGE_TOKEN_BUDGETS` — request envelope.
- `Response { Token, Done, Error, Status }` — streamed response
  frames. `Done` carries `stop_reason` + `backend`; `Error` carries
  a structured `code` (`queue_full`, `backend_unavailable`,
  `invalid_request`, `internal`).
- `read_frame` / `write_frame` — bounded NDJSON framing. 64 MiB
  per-line cap is non-negotiable.
- `Usage`, `StopReason`, `ErrorCode` — small enums round out the
  shape.

## Versioning

Protocol v1 is **frozen** as of [ADR 0008](https://github.com/3rg0n/inferd/blob/main/docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md).
Backwards-additive changes within v1 are acceptable (new optional
fields older parsers ignore); breaking changes go to v2 on a
**separate socket path**, not in-band negotiation.

`inferd-proto 0.1` matches `inferd-daemon 0.1` and `inferd-client 0.1`.

## Usage

Most consumers want [`inferd-client`](https://crates.io/crates/inferd-client),
which re-exports the wire types from this crate so a client doesn't
need to depend on both. Pull `inferd-proto` directly only if you're
building a non-client tool that needs to parse or generate frames
(e.g. a sidecar HTTP→IPC adapter).

```toml
[dependencies]
inferd-proto = "0.1"
```

## License

MIT. See `LICENSE`.

## Contributing

Bug reports, design discussions, and PRs welcome at
[github.com/3rg0n/inferd](https://github.com/3rg0n/inferd). Read
`CONTRIBUTING.md` in the upstream repo before opening a PR.
