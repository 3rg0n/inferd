# 0021. Unify on one generation API; length-prefixed, type-tagged framing with raw BLOB media and in-band wire version

- Status: accepted
- Date: 2026-06-06
- Supersedes: parts of [0008](0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md)
  (v1 as a separate frozen surface), [0009](0009-pre-m1-open-questions-resolved.md)
  (no in-band version negotiation), and [0015](0015-v2-wire-protocol-typed-content-blocks.md)
  (newline-delimited v2 framing).

## Context

inferd is still pre-launch. The only consumer is thlibo, a first-party
Go middleware with no end users of its own, and the Rust daemon. We
control both ends. v0.3.0 published `inferd-proto` / `inferd-client`
0.3.0 to crates.io and a `go get`-able Go v2 client, all speaking the
current **newline-delimited JSON** v2 framing — but nothing in
production consumes them yet. This is the cheapest moment to fix the
wire format before a consumer ossifies around it; after real launch,
framing changes become breaking in a way that matters.

Three problems with the shipped framing (issue #34, after reviewing
OpenAI/Anthropic/Bedrock surfaces, llama.cpp's server API, the Abilian
IPC/Varlink survey, and the gRPC/Cap'n-Proto/shm tradeoffs):

1. **Newline-delimited JSON cannot carry raw binary** — a `0x0A` byte
   in image data splits the frame. That is *why* media is base64 today.
2. **Base64-in-JSON tax on the hot path.** A rasterized PDF page
   (~1500×2000×3 ≈ 9 MB raw RGB) becomes ~12 MB of base64 in a single
   line: +33% size, encode+decode CPU on every vision request, and the
   whole line must be buffered + line-scanned before the daemon can
   act. It also forces `MaxFrameBytes` to accommodate multi-MB image
   lines, so the cap can't protect the tiny control frames.
3. **Schema drift is silent.** The v1→v2 Go-client gap was undetectable
   — a client with no v2 types just had no v2 types. Nothing failed
   loudly.

Separately, the project carries two *generation* surfaces (v1 text-only
on `infer.sock`, v2 typed-content on `infer.v2.sock`). For a
single-machine project with one controlled consumer this is incidental
version churn presented as two APIs.

## Decision

Because we are pre-launch with consumers we control on both ends, we
**mutate the v2 surface in place** rather than stand up a successor
socket — explicitly overriding the "frozen surface / separate socket
per version" posture of ADRs 0008/0009/0015 for this one pre-launch
window. After v0.4 ships and real consumers exist, the freeze
re-applies in full: subsequent breaking changes go to a successor
socket, never an in-place mutation.

Four changes, landing together in v0.4:

### 1. One generation API

Fold v1 into v2. The v1 text-only socket (`infer.sock` /
`\\.\pipe\inferd-infer`) is **removed**. All generation goes through
the v2 typed-content surface; a text-only request is a v2 request whose
content is a single `text` block. v1-vs-v2 was incidental version
churn, not two real APIs.

`embed` stays a **separate operation** on its own socket (ADR 0017
unchanged) — it is universal across providers and is not a modality of
generate. It adopts the same framing (below) for consistency.

### 2. Length-prefixed, type-tagged framing

Replace newline-delimited framing with:

```
[uvarint payload_len][1 byte frame_type][payload]
  frame_type = 0x01 JSON  → a UTF-8 JSON control frame (today's shapes)
  frame_type = 0x02 BLOB  → raw bytes, keyed by attachment id from a prior JSON frame
```

- `payload_len` is an unsigned LEB128 varint counting the bytes of
  `payload` only (it does **not** include the type byte). The 64 MiB
  cap (THREAT_MODEL F-5) applies to `payload_len`; a length that
  exceeds the cap is rejected before any payload bytes are read.
- Control frames keep **exactly today's JSON shapes** (RequestV2,
  the `frame`/`done`/`error` responses, capabilities) — still
  greppable, zero-codegen, just length-prefixed instead of
  newline-terminated.

### 3. Media rides as a raw BLOB frame

A request carrying attachments sends, in order: one JSON frame (the
`RequestV2` with attachment *metadata* + ids, **no bytes**), then one
BLOB frame per attachment, each preceded by a tiny JSON descriptor
frame naming the `attachment_id` and byte length it applies to so the
reader can correlate without guessing order. The daemon hands the raw
bytes straight to mtmd — zero-copy on the read side, no base64.

`AttachmentV2.bytes` (the base64 string) is **removed** from the JSON.
The JSON attachment object keeps `kind` / `id` / `width` / `height` /
`sample_rate`. This is still fully consistent with ADR 0016 (consumer
decodes media → raw RGB; daemon links no codec) — only the *transport*
changes, not the decode posture.

### 4. In-band wire version

Add `wire_version` (an integer, starting at `1` for the v0.4 framing)
to the request frame, and surface the daemon's supported `wire_version`
in the **capabilities frame** alongside `vision/audio/embed/n_ctx`. A
request whose `wire_version` the daemon does not support is rejected
with a clear `wire_version_unsupported` error naming both versions.

This reverses ADR 0009's "no in-band negotiation" — justified because
folding to one socket removes the separate-socket-per-version mechanism
that made in-band negotiation unnecessary. With one generation socket,
a version tag is how mismatches fail loudly instead of silently.

## Why not gRPC / protobuf / Cap'n Proto / shared memory on the IPC wire

- **gRPC** = HTTP/2 + protobuf + codegen; drags HTTP back onto the pipe
  we keep dialect-free, and named-pipe support is not first-class
  outside .NET (our hardest platform, Windows, needs custom dialers in
  grpc-go/tonic). Justified for distributed/multi-team/many-language
  APIs; we are single-machine with two first-party consumers.
- **protobuf alone**: marginal CPU win on control frames that are
  already tiny, at the cost of the debuggability that has been
  load-bearing (it saved us during the v1/v2 client-mismatch work).
- **Cap'n Proto / shared memory**: the zero-copy tier is for
  microsecond-critical *structured* data (video, HFT). Our media is
  unstructured raw RGB sent occasionally (per-document OCR), for which
  a raw BLOB frame is already zero-copy. Shared memory is the right
  *future* escalation (pass an shm handle in the JSON control frame) if
  media throughput ever becomes a measured bottleneck — not now.

HTTP dialects (OpenAI Chat-Completions, Anthropic Messages) belong in
the outward `inferd-http` bridge (#33, ADR 0020), never on the pipe.
Unifying generation on v2 first means each dialect translates into one
target, not N.

## Consequences

**Easier:**
- Media is zero-copy and base64-free; the per-frame cap protects tiny
  control frames again instead of being sized for multi-MB image lines.
- One generation API to document, test, and translate HTTP dialects
  into.
- Version mismatches fail loudly with a named error.

**Harder / costs:**
- Breaking change. The v0.3.0 published `inferd-proto` / `inferd-client`
  crates and the Go v2 client speak the old newline framing and will
  **not** interoperate with a v0.4 daemon. This is acceptable *only*
  because we are pre-launch with consumers we control; the v0.3.0
  crates stay on crates.io for anyone pinned to that line. v0.4 bumps
  the minor to signal the break.
- Rust `inferd-proto` (framing + RequestV2/Attachment), `inferd-daemon`
  (lifecycle read/write loops, drop the v1 socket), and the Go client
  must move in lockstep and stay byte-compatible.
- A frame-dump debug tool must understand the type tag (JSON frames stay
  UTF-8 JSON and greppable; BLOB frames are opaque by design).

**Unchanged:**
- ADR 0006 (no HTTP in the daemon), ADR 0007 (routing / no in-daemon
  retry), ADR 0010 (narrow HTTPS for model bootstrap), ADR 0011 (CAS
  store), ADR 0012 (one warm model), ADR 0013 (gateway shaping), ADR
  0016 (consumer decodes media), ADR 0017 (embed is its own socket),
  ADR 0019 (runtime accelerator detection), ADR 0020 (HTTP bridge is a
  separate process).
- The post-launch freeze posture **returns** after v0.4: this ADR is a
  one-time pre-launch correction, not a license for future in-place
  wire mutations.

## Acceptance (issue #34)

- Length-prefixed, type-tagged framing replaces newline-delimited on the
  generation socket.
- Image/audio/video attachments carry raw bytes in BLOB frames;
  `AttachmentV2.bytes` base64 field removed.
- `wire_version` on the request and in the capabilities frame; mismatch
  produces a clear error.
- Go client (`clients/go`) + Rust `inferd-proto` updated in lockstep,
  byte-compatible.
- v1 folded into v2 (v1 socket removed); text-only is a single-text-block
  v2 request.
- Debuggability preserved: JSON frames stay UTF-8 JSON; only BLOB frames
  are opaque.

## References

- Issue #34 — the design this ADR ratifies.
- ADR 0008 / 0009 / 0015 — superseded in part (see header).
- ADR 0016 — consumer-decoded media (decode posture unchanged).
- ADR 0017 — embed on its own socket (unchanged; adopts the new framing).
- ADR 0020 — HTTP dialects live in the inferd-http bridge.
