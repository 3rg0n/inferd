# 0025. The bridge decodes and resamples audio; codecs stay out of the daemon

- Status: accepted
- Date: 2026-08-03
- Extends: [0016](0016-consumer-decodes-media-before-sending.md) (consumer decodes media), [0020](0020-inferd-http-bridge-is-a-separate-process.md) (bridge is a separate process)

## Context

The daemon's audio path shipped in v0.6.1 and was proven live in v0.6.2-dev
(task #199): a v2 `Attachment::Audio` carries mono little-endian float32 PCM
plus a `sample_rate`, the daemon hands the samples to
`mtmd_bitmap_init_from_audio`, and Gemma 4's audio encoder transcribes them.
ADR 0016 already settled who decodes: the consumer. The daemon links no codec.

What #199 also established is that **the sample rate is a hard contract, not a
hint**. libmtmd's audio entry point takes a bare `&[f32]` with no rate
argument — the encoder consumes samples at whatever rate it was trained for.
Feeding 44.1 kHz into Gemma 4 E4B's 16 kHz encoder time-scales the audio
≈2.75× and returns a *fluent, wrong* answer with nothing in the bytes to
reveal it. So the daemon now advertises its backend's required rate as
`audio_sample_rate` on the admin capabilities frame and **rejects** any
attachment at a different rate rather than silently resampling.

That left the OpenAI-compat bridge (`inferd-http`) unable to accept audio at
all. OpenAI's `input_audio` content part carries base64 wav/mp3 at whatever
rate the client recorded — 44.1 kHz and 48 kHz are the overwhelmingly common
cases, 16 kHz is rare. A bridge that forwards those bytes untouched produces
exactly two outcomes, both bad: a daemon rejection for every realistic client,
or (if it guessed a rate) a confidently wrong transcription.

Three placements were available for the decode-and-resample work: the daemon,
the bridge, or the OpenAI client. The third is not available in practice — the
whole point of an OpenAI-compat surface is that unmodified SDK clients work
against it, and no OpenAI SDK resamples audio.

## Decision

**`inferd-http` decodes and resamples audio. The daemon still links no codec
and still never resamples.**

Concretely, in `crates/inferd-http/src/audio_decode.rs`:

1. Decode the base64 `input_audio.data` with `symphonia` (wav + mp3 + pcm),
   downmixing to mono **per packet** so peak memory is independent of the
   source's channel count.
2. Resample to the target rate with `rubato`'s FFT resampler. A no-op when the
   source already matches, which is the fast path for a well-behaved client.
3. Serialize to mono LE-f32 and attach as `Attachment::Audio { sample_rate }`
   with the target rate.

**The target rate is read from the daemon, not hardcoded.** The bridge dials
the admin socket, reads the capabilities frames, and takes the
`audio_sample_rate` of the first backend advertising `audio: true`. This is
done **per request that carries audio**, not cached at startup: the daemon can
restart onto a different mmproj with a different rate, and a cached value would
then produce precisely the confidently-wrong output the rate contract exists to
prevent. Text-only and image-only requests never pay for the extra connect. If
no backend advertises a rate, the bridge returns HTTP 400 (`AudioUnsupported`)
rather than guessing.

Consequential additions:

- `inferd-http` gains `--admin-addr-override` (the third daemon endpoint it now
  needs) and two new dependencies, `symphonia` and `rubato`.
- `inferd-client` re-exports `AdminError`, which was already the error type of
  a public API but had never been exported.
- The bridge's per-request decoded-attachment byte budget becomes **shared
  across modalities** (`MAX_TOTAL_DECODED_ATTACHMENT_BYTES`, 128 MiB, renamed
  from the image-only constant) to mirror the daemon's own aggregate
  `MAX_ATTACHMENT_BYTES_PER_REQUEST`. Two independent per-modality budgets
  would let the bridge build a request the daemon then refuses.
- Audio clips are capped at `MAX_AUDIO_CLIPS_PER_REQUEST` = 4 per request,
  alongside the existing image cap of 8. Both stay under the proto layer's
  `MAX_ATTACHMENTS_PER_REQUEST` = 32, enforced by a compile-time assert.

### On the MPL-2.0 dependency

`symphonia` and its subcrates are **MPL-2.0** — the first non-permissive
dependency in inferd's tree. This is accepted, with the scope stated
explicitly:

- MPL-2.0 is **file-level** copyleft. It reaches modifications to symphonia's
  own source files. It does not reach a separate work that merely links the
  library, which is exactly inferd's relationship to it (§3.3, "Larger Work").
- The dependency is confined to **`inferd-http`**, which is a binary crate and
  is **not published to crates.io**. The daemon (`inferd-daemon`), the engine
  (`inferd-engine`), and both published library crates (`inferd-proto`,
  `inferd-client`) do not link it. A downstream Rust consumer taking a
  dependency on inferd's libraries pulls in nothing MPL.
- inferd stays MIT (ADR 0004). The `inferd-http` binary is an MIT work that
  links an MPL-2.0 library, and its distribution carries symphonia's notice.

The alternative permissive audio decoders were surveyed and none covers mp3
plus wav with comparable maintenance; `symphonia` is the de-facto Rust audio
decoder and is what the ecosystem (rodio, etc.) already standardises on.

## Consequences

**Easier:**

- OpenAI SDK clients can send `input_audio` at whatever rate they recorded —
  the common case, and the only thing that makes the bridge's audio support
  honest rather than nominal.
- The daemon is unchanged. No new wire fields, no `wire_version` bump, no new
  daemon dependency, and the rate contract stays a hard reject.
- Rate discovery is dynamic, so swapping the model or mmproj under a running
  bridge cannot desynchronise the two.

**Harder:**

- The bridge grew a real codec surface: ~500 lines plus two dependency trees,
  and audio decoders are a historically CVE-prone class. Bounded by three
  explicit caps (encoded payload size, accumulated decoded sample count checked
  *during* the decode loop so a bomb fails partway, and predicted resampled
  payload size checked before the work happens), and by the fact that the
  bridge is an unprivileged user-launched process (ADR 0014/0020) rather than
  the daemon.
- An audio request costs one extra daemon connect for the rate probe. Bounded
  by a 5 s timeout and skipped entirely for non-audio requests.
- The repo now needs a licence-posture statement, since "everything is
  permissive" stopped being true. `cargo deny` is in the pre-commit gate and is
  the place to encode an allow-list.

**What this explicitly does not change:**

- ADR 0016 — strengthens it. The consumer still decodes; this ADR just names
  the bridge as one such consumer and extends "decode" to include rate
  conversion.
- ADR 0006 (lean core) — strengthens it. The codec landed in an
  ecosystem-extension process, which is exactly where ADR 0006 puts this class
  of work.
- ADR 0004 (MIT) — unchanged, with the containment argument above.
- The v2 wire — unchanged. `Attachment::Audio` already carried `sample_rate`.

## Alternatives considered

- **Daemon decodes and resamples.** Rejected: it is the position ADR 0016
  already rejected for images, for the same reasons (contradicts ADR 0013,
  violates ADR 0006, and here it would also put MPL-2.0 code and a CVE-prone
  decoder inside the privileged, always-running process).
- **Daemon resamples but does not decode** (accept LE-f32 at any rate, convert
  internally). Rejected: it re-opens the failure mode #198/#199 closed. Silent
  rate conversion inside the daemon means a consumer that miscomputed its rate
  gets a plausible answer instead of an error, and the daemon would then own a
  DSP correctness surface for every consumer, not just OpenAI clients.
- **Bridge hardcodes 16000.** Rejected: correct only for the current default
  mmproj. A different mmproj, or the 12B variant, would silently produce
  time-scaled garbage — the exact class of bug the rate contract exists to
  make impossible.
- **Bridge caches the rate at startup.** Rejected for the same reason at a
  longer time horizon: a daemon restart onto a different model desynchronises
  it, and the symptom is a wrong answer rather than an error. The per-request
  probe is cheap and only paid by audio requests.
- **Reject non-target-rate audio at the bridge with a helpful 400.** Rejected
  as nominal support: no OpenAI SDK resamples, so every realistic client would
  get the error. "The feature exists but nothing can use it" is not a feature.

## References

- ADR 0004 — MIT, not Apache-2.0 (licence posture).
- ADR 0006 — lean core; codecs belong in ecosystem extensions.
- ADR 0013 — gateway, not pipe.
- ADR 0016 — consumer decodes media; daemon stays codec-free.
- ADR 0020 — the bridge is a separate, user-launched process.
- `crates/inferd-proto/src/v2/attachment.rs` — the audio rate contract and the
  attachment caps.
- Task #198 (advertise + reject on rate mismatch), #199 (live audio
  validation), #200 (this change).
- libmtmd C ABI: `mtmd_bitmap_init_from_audio(n_samples, f32_data)` — no rate
  argument, which is the root of the whole contract.
