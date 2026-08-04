# inferd-proto

Wire format types and framing for [inferd](https://github.com/3rg0n/inferd),
a local-inference daemon. This crate is the canonical schema reference
for clients in any language: Rust clients depend on it directly;
Go/Python/TypeScript clients use it as the source of truth.

As of v0.4 the generation surface uses length-prefixed, type-tagged
frames per [ADR 0021](https://github.com/3rg0n/inferd/blob/main/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md);
the v2 content shape is specified in ADR 0015, embeddings in ADR 0017,
and rerank in ADR 0027. This crate carries the `serde`-derived Rust
types plus the frame codecs with the 64 MiB per-frame cap
(THREAT_MODEL F-5).

## What's in here

- `v2::{RequestV2, ResolvedV2, MessageV2, RoleV2, ContentBlock,
  ResponseV2, ResponseBlock, Attachment, BlobDescriptor, Tool,
  ResponseFormat, StopReasonV2, UsageV2, ErrorCodeV2, WIRE_VERSION}` —
  the single generation surface (typed content blocks, attachments,
  tools, in-band `wire_version`). `RequestV2.response_format`
  (`json_schema`, v0.5.0) and `RequestV2.thinking` (reasoning
  activation, v0.5.1) are optional, backwards-additive fields.
- `embed::{EmbedRequest, EmbedResolved, EmbedResponse, EmbedTask,
  EmbedUsage, EmbedErrorCode}` — the embeddings surface (ADR 0017).
- `rerank::{RerankRequest, RerankResolved, RerankResponse, RerankResult,
  RerankUsage, RerankErrorCode, MAX_RERANK_DOCUMENTS,
  MAX_RERANK_TOTAL_BYTES}` — the cross-encoder rerank surface
  ([ADR 0027](https://github.com/3rg0n/inferd/blob/main/docs/adr/0027-reranking-on-a-fourth-socket.md)).
  Single-frame request, single-frame response; `results` carry the
  caller's document `index` and a **raw** score, never document text.
- `frame.rs`: `read_lp_frame` / `write_lp_json` / `write_lp_blob` +
  `FrameType` / `RawFrame` — the length-prefixed type-tagged codec for
  generation; `read_frame` / `write_frame` — bounded NDJSON for embed
  and rerank. `MAX_FRAME_BYTES` (64 MiB) is non-negotiable.
- `ErrorCode`, `ProtoError` — shared error types.

The text-only v1 types (`Request`/`Response`/`Resolved`/`Role`/
`Message`/`StopReason`/`Usage`/`ImageTokenBudget`) were removed in v0.4
when v1 was folded into v2.

## Attachments carry decoded bytes, not encoded files

The daemon links no image or audio codec ([ADR 0016](https://github.com/3rg0n/inferd/blob/main/docs/adr/0016-consumer-decodes-media-before-sending.md)),
so an `Attachment` is always already-decoded samples in a BLOB frame —
never a PNG, JPEG, WAV or MP3. Two forms:

| Variant | Descriptor fields | BLOB payload |
|---|---|---|
| `Attachment::Image` | `id`, `width`, `height` | raw RGB, `width * height * 3` bytes |
| `Attachment::Audio` | `id`, `sample_rate` (Hz) | mono **little-endian float32** PCM samples |

Two contracts a client must get right, because neither fails safe:

- **Endianness is explicit, not native.** Audio samples are
  little-endian on every platform. Encoding with native byte order
  happens to work on x86_64 and arm64 and would ship a latent bug for
  a big-endian consumer.
- **`sample_rate` MUST equal the backend's advertised
  `audio_sample_rate`**, read off the admin `capabilities` frame (16000
  for the reference Gemma 4 E4B mmproj). The daemon **rejects** a
  mismatch and never resamples: libmtmd's audio entry point takes no
  rate argument, so the wrong rate is not a detectable error — it
  time-scales the clip and yields a fluent *wrong* answer. Read the
  rate per request rather than caching it; a daemon restart can land on
  a different mmproj.

Per-request bounds (v0.6.1): 32 attachments and 128 MiB of them
aggregate, independent of the 64 MiB single-frame cap.

Converting encoded media is the consumer's job. `inferd-http` is the
reference implementation — it decodes wav/mp3, downmixes to mono, and
resamples to whatever rate the daemon reports
([ADR 0025](https://github.com/3rg0n/inferd/blob/main/docs/adr/0025-bridge-decodes-and-resamples-audio.md)).

## Rerank scores are raw, and the bounds are on count, not bytes

A `RerankResult.score` is the reranker's raw classification logit. It is
**not** normalised, **not** a probability, and comparable only *within
one response* — negative values are ordinary, and the scale differs per
model. Sort/threshold within a response; never persist a score or
compare across two.

`MAX_RERANK_DOCUMENTS` (256) and `MAX_RERANK_TOTAL_BYTES` (8 MiB) are
enforced at parse time, in addition to the frame cap, because the frame
cap bounds the wrong thing here: rerank runs one forward pass per
document, so a single 64 MiB-legal frame could ask for thousands of
them. That is the same amplification class as the per-request attachment
bounds above (THREAT_MODEL F-1).

## Versioning

The generation (v2) wire is **frozen** as of
[ADR 0021](https://github.com/3rg0n/inferd/blob/main/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md);
embeddings as of ADR 0017; rerank as of ADR 0027. Backwards-additive
changes are acceptable
(new optional fields older parsers ignore); a breaking change to the
generation wire bumps the in-band `WIRE_VERSION` so a mismatch fails
loudly rather than negotiating silently.

`inferd-proto 0.6.x` matches `inferd-daemon 0.6.x` and
`inferd-client 0.6.x`; the published patch versions move in lockstep.

## Usage

Most consumers want [`inferd-client`](https://crates.io/crates/inferd-client),
which re-exports the wire types from this crate so a client doesn't
need to depend on both. Pull `inferd-proto` directly only if you're
building a non-client tool that needs to parse or generate frames
(e.g. a sidecar HTTP→IPC adapter).

```toml
[dependencies]
inferd-proto = "0.6"
```

## License

MIT. See `LICENSE`.

## Contributing

Bug reports, design discussions, and PRs welcome at
[github.com/3rg0n/inferd](https://github.com/3rg0n/inferd). Read
`CONTRIBUTING.md` in the upstream repo before opening a PR.
