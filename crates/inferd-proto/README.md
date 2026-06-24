# inferd-proto

Wire format types and framing for [inferd](https://github.com/3rg0n/inferd),
a local-inference daemon. This crate is the canonical schema reference
for clients in any language: Rust clients depend on it directly;
Go/Python/TypeScript clients use it as the source of truth.

As of v0.4 the generation surface uses length-prefixed, type-tagged
frames per [ADR 0021](https://github.com/3rg0n/inferd/blob/main/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md);
the v2 content shape is specified in ADR 0015 and embeddings in
ADR 0017. This crate carries the `serde`-derived Rust types plus the
frame codecs with the 64 MiB per-frame cap (THREAT_MODEL F-5).

## What's in here

- `v2::{RequestV2, ResolvedV2, MessageV2, RoleV2, ContentBlock,
  ResponseV2, ResponseBlock, Attachment, BlobDescriptor, Tool,
  StopReasonV2, UsageV2, ErrorCodeV2, WIRE_VERSION}` — the single
  generation surface (typed content blocks, attachments, tools,
  in-band `wire_version`).
- `embed::{EmbedRequest, EmbedResolved, EmbedResponse, EmbedTask,
  EmbedUsage, EmbedErrorCode}` — the embeddings surface (ADR 0017).
- `frame.rs`: `read_lp_frame` / `write_lp_json` / `write_lp_blob` +
  `FrameType` / `RawFrame` — the length-prefixed type-tagged codec for
  generation; `read_frame` / `write_frame` — bounded NDJSON for embed.
  `MAX_FRAME_BYTES` (64 MiB) is non-negotiable.
- `ErrorCode`, `ProtoError` — shared error types.

The text-only v1 types (`Request`/`Response`/`Resolved`/`Role`/
`Message`/`StopReason`/`Usage`/`ImageTokenBudget`) were removed in v0.4
when v1 was folded into v2.

## Versioning

The generation (v2) wire is **frozen** as of
[ADR 0021](https://github.com/3rg0n/inferd/blob/main/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md);
embeddings as of ADR 0017. Backwards-additive changes are acceptable
(new optional fields older parsers ignore); a breaking change to the
generation wire bumps the in-band `WIRE_VERSION` so a mismatch fails
loudly rather than negotiating silently.

`inferd-proto 0.4` matches `inferd-daemon 0.4` and `inferd-client 0.4`.

## Usage

Most consumers want [`inferd-client`](https://crates.io/crates/inferd-client),
which re-exports the wire types from this crate so a client doesn't
need to depend on both. Pull `inferd-proto` directly only if you're
building a non-client tool that needs to parse or generate frames
(e.g. a sidecar HTTP→IPC adapter).

```toml
[dependencies]
inferd-proto = "0.5"
```

## License

MIT. See `LICENSE`.

## Contributing

Bug reports, design discussions, and PRs welcome at
[github.com/3rg0n/inferd](https://github.com/3rg0n/inferd). Read
`CONTRIBUTING.md` in the upstream repo before opening a PR.
