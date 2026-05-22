# 0016. Consumer decodes media before sending — daemon stays codec-free

- Status: accepted
- Date: 2026-05-20
- Amends: [0015](0015-v2-wire-protocol-typed-content-blocks.md) §"v2 Attachment"

## Context

ADR 0015 §"v2 Attachment" specified that v2 attachments carry
arbitrary encoded bytes (JPEG, PNG, WAV, MP3, etc.) along with a
MIME hint, with the daemon responsible for decoding them before
handing raw RGB / float32 PCM to libmtmd. Two days into Phase 3A
implementation, that specification ran into three problems:

1. **It contradicts ADR 0013.** The gateway framing says
   middleware owns the bytes; the daemon owns model-specific
   shaping. Image format is a middleware concern (JPEG vs. PNG vs.
   WebP), not a model-shape concern. Putting it in the daemon
   widens the daemon's surface in exactly the direction ADR 0013
   said it shouldn't.
2. **It violates ADR 0006 (lean core).** Decoding JPEG / PNG / WebP
   pulls in `image` (or `zune-jpeg` / `png`) plus their
   transitive surface. Decoding WAV / MP3 / FLAC pulls in
   `symphonia` or similar. That's roughly 5–15 MB of binary
   growth and a substantial supply-chain surface (CVEs in
   `image` and audio crates are not rare).
3. **It double-encodes.** Real consumers — `inferd` CLI, thlibo,
   middleware integrations like Claude Code — already have a
   decoded representation in memory by the time they think about
   sending it (because they had to read the file and decide
   whether to display, resize, or transform it before invoking
   inferd). Sending raw RGB + dimensions matches what the
   consumer already has, not what they'd have to re-encode.

libmtmd's C ABI accepts exactly the raw forms: `mtmd_bitmap_init`
takes `nx * ny * 3` interleaved RGB octets,
`mtmd_bitmap_init_from_audio` takes a float32 PCM slice. So the
encoded-on-the-wire path inside the daemon was always going to be
"decode then re-call mtmd with the raw form" — pure overhead.

## Decision

Consumers decode media before sending it on the v2 wire. The
daemon never links image or audio codec libraries.

The `Attachment` type in `inferd-proto::v2` becomes a
serde-tagged enum with one variant per modality, each carrying
the metadata that modality requires:

```rust
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Attachment {
    Image {
        id: String,
        width: u32,
        height: u32,
        bytes: String,           // base64 of width * height * 3 RGB octets
    },
    Audio {
        id: String,
        sample_rate: u32,        // Hz
        bytes: String,           // base64 of f32 PCM samples (LE)
    },
    Video {
        id: String,
        bytes: String,           // shape TBD when a video adapter ships
    },
    #[serde(other)]
    Unknown,                     // forward-compat escape hatch
}
```

`AttachmentKind` (the discriminant enum from the original ADR
0015 design) is removed — the discriminant is implicit in the
serde tag.

The `mime` field is removed entirely. With the modality known
from the variant tag and the encoding fixed (raw RGB, raw f32
PCM), there's nothing for MIME to communicate.

`RequestV2::resolve()` is tightened to verify *kind correspondence*:
a `ContentBlock::Image { attachment_id }` must reference an
`Attachment::Image`. Mismatches return `InvalidRequest` early —
they don't reach the engine adapter.

Future modalities (a hypothetical `Document` block, etc.) add new
variants to `Attachment` and matching `ContentBlock` block types.
Older clients see `Attachment::Unknown` and reject at `resolve()`.

## Consequences

**Easier:**

- Daemon stays text-shaped (no JPEG decoder, no audio decoder).
  Smaller binary, smaller dependency tree, smaller threat
  surface.
- Phase 3A's mtmd FFI bridge maps directly: `Attachment::Image
  { width, height, bytes }` → base64-decode → `mtmd_bitmap_init(
  width, height, decoded_bytes_ptr)`. No intermediate decode
  step, no per-format branch.
- Each `Attachment` variant carries exactly the metadata the
  matching mtmd FFI call needs. Type-driven; you can't construct
  an audio attachment without a sample rate.
- The chat-template renderer (Phase 2B) doesn't need to know how
  to flatten variant-specific fields — it only needs the id and
  whether to emit a media marker.

**Harder:**

- Middleware authors who didn't already have a decoded image in
  memory (e.g. they're forwarding a file path) need to decode
  before sending. We provide reference middleware (the `inferd`
  CLI) that does this with `image` so authors can copy the
  pattern.
- A consumer-side bug (sending RGBA bytes when the wire expects
  RGB) is detected by mtmd, not the daemon's validation layer.
  The error reaches the consumer as
  `Error{AttachmentUnsupported, ...}` from the engine adapter
  rather than `InvalidRequest` from the proto layer. Acceptable;
  the price for not pulling codecs into the daemon.

**What this explicitly does not change:**

- ADR 0006 (lean core) — strengthens it.
- ADR 0008 (v1 frozen) — v1's `image_token_budget` flag stays as
  it is; this ADR only redesigns the v2 attachment shape.
- ADR 0013 (gateway framing) — strengthens it. Middleware owns
  decoding; daemon owns shaping.
- ADR 0015 §"v2 Request", §"v2 ContentBlock variants",
  §"Response frames" — unchanged. Only the `Attachment` shape
  changes.

## Alternatives considered

- **Keep ADR 0015 as written; daemon decodes.** Rejected per the
  three problems above.
- **Both shapes on the wire, MIME determines.** Rejected — two
  code paths, two test matrices, two threat-model legs. The
  forward-compat escape hatch (`Attachment::Unknown`) handles
  unknown future modalities; we don't need a flexible-encoding
  shim today.
- **Keep `mime` as a hint for future modalities.** Rejected as
  carrying no information today. If a future ADR adds a modality
  where MIME is meaningful, that ADR's variant adds whatever
  metadata fields it needs — same pattern as the current ones.

## References

- ADR 0006 — lean core posture (this ADR strengthens it).
- ADR 0013 — gateway framing (this ADR strengthens it).
- ADR 0015 — v2 wire protocol (this ADR amends §"v2 Attachment").
- libmtmd C ABI: `mtmd_bitmap_init(nx, ny, rgb_data)`,
  `mtmd_bitmap_init_from_audio(n_samples, f32_data)` —
  `vendor/llama.cpp/tools/mtmd/mtmd.h`.
