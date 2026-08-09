# inferd-http

OpenAI-compatible HTTP bridge for the [inferd](https://github.com/3rg0n/inferd)
local-inference daemon ([ADR 0020](../../docs/adr/0020-inferd-http-bridge-is-a-separate-process.md)
Surface A). Point OpenCode — or any OpenAI-SDK client — at this server
and it talks to your local inferd daemon.

It is a **separate, user-launched process** and a **consumer, not a
privileged surface** ([ADR 0014](../../docs/adr/0014-inferd-cli-is-a-reference-middleware.md)):
it holds no model, does no inference, and reaches the daemon over the
same IPC every other consumer uses (`inferd-client`). The daemon never
serves HTTP ([ADR 0006](../../docs/adr/0006-lean-core-ecosystem-extensions.md)/[0022](../../docs/adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md)).

## Endpoints

- `POST /v1/chat/completions` — streaming (SSE) and non-streaming;
  text, vision (`image_url` content parts) **and** audio (`input_audio`),
  see below.
- `POST /v1/embeddings` — `float` and `base64` encodings (the OpenAI SDK
  defaults to `base64`).
- `GET /v1/models` — advertises the single warm model.
- `GET /health` — liveness.

## Vision (image input)

The bridge accepts OpenAI multimodal chat content — a `user` message
whose `content` is an array of parts, mixing `text` and `image_url`:

```python
client.chat.completions.create(
    model="anything",
    messages=[{
        "role": "user",
        "content": [
            {"type": "text", "text": "What does this say?"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,<...>"}},
        ],
    }],
)
```

The daemon links no image codec (ADR 0016): the bridge decodes the
PNG/JPEG to raw RGB and sends it as an inferd image attachment over the
BLOB-frame wire. Requires the daemon to have a vision-capable model warm
(the default Gemma 4 E4B is); a non-vision model returns the daemon's
`attachment_unsupported` error.

- **`data:` URLs only.** A remote `http(s)://` image URL is rejected with
  a 400 — a server-side fetch of an arbitrary URL is an SSRF vector, so
  the bridge does not fetch. Inline the image as a base64 `data:` URL
  (which is what the OpenAI SDK does when you pass image bytes).
- **Bomb-guarded.** The encoded payload is capped (8 MiB) and decode is
  bounded by max dimensions (8192²) and a max-allocation limit, so a
  small hostile image can't exhaust memory.
- Images are accepted only on `user` messages. The `detail` hint is
  accepted and ignored (inferd's image budget is an operator/model
  property — `mmproj_image_max_tokens` — not a per-request knob).

## Audio (speech input)

The bridge accepts OpenAI's `input_audio` content part — inline base64
wav or mp3 on a `user` message:

```python
client.chat.completions.create(
    model="anything",
    messages=[{
        "role": "user",
        "content": [
            {"type": "text", "text": "Transcribe this."},
            {"type": "input_audio", "input_audio": {"data": "<base64 wav>", "format": "wav"}},
        ],
    }],
)
```

**Send audio at any sample rate — the bridge converts it.** That is the
whole reason this path exists ([ADR 0025](../../docs/adr/0025-bridge-decodes-and-resamples-audio.md)).
The daemon's audio encoder takes mono float32 PCM at *one* rate and
**rejects** anything else rather than resampling, because the wrong rate
time-scales the audio and yields a fluent, wrong answer instead of an
error. So the bridge decodes your wav/mp3, downmixes to mono, and
resamples to the rate the daemon advertises.

- **The rate comes from the daemon, per request**, read off its admin
  socket (`--admin-addr-override` if it isn't at the platform default) —
  never hardcoded, so restarting the daemon onto a different model can't
  desynchronise the two. Only requests that actually carry audio pay for
  that extra connect.
- Requires an audio-capable model warm (the default Gemma 4 E4B is). If no
  backend takes audio you get a 400 rather than a guessed rate the daemon
  would reject anyway. A daemon that reports audio but no *rate* is a
  **daemon older than v0.6.2** — that gets its own 400 telling you to
  upgrade, since it's a different fix from swapping the model.
- **No URL form.** OpenAI defines none for audio, so there is nothing to
  fetch and no SSRF surface (unlike `image_url`).
- **Bomb-guarded.** Three caps: encoded payload (8 MiB), decoded sample
  count (checked *during* decode, so a small mp3 claiming hours of audio
  fails partway), and the resampled payload size (checked from the
  predicted length, before the work happens).
- Audio is accepted only on `user` messages, max 4 clips per request, and
  shares a 128 MiB decoded-attachment budget with images.
- Formats: **wav** and **mp3**. The `format` field is a hint only —
  detection is from the bytes, so a wrong hint costs nothing.

## Run

```sh
# The inferd daemon must be running locally first.
inferd-http                       # binds 127.0.0.1:8080, no auth
inferd-http --listen 127.0.0.1:9000
inferd-http --model-name my-model # what /v1/models advertises + echoes
```

Endpoint overrides, if the daemon isn't at the platform defaults:

```sh
inferd-http --gen-addr-override   /run/user/1000/inferd/inferd.sock \
            --embed-addr-override /run/user/1000/inferd/infer.embed.sock \
            --admin-addr-override /run/user/1000/inferd/inferd.admin.sock
```

Then point a client at it:

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="not-needed")
client.chat.completions.create(
    model="anything",             # accepted + echoed; inferd serves one warm model
    messages=[{"role": "user", "content": "hello"}],
)
```

## Security

- **Localhost, no auth by default.** Fine for a user running local tools.
- **Non-loopback bind requires `--token`.** The bridge refuses to bind a
  network-reachable address without a bearer token; requests must then
  send `Authorization: Bearer <token>`. TLS terminates in front (a
  reverse proxy) — the bridge speaks plain HTTP.
- Auth terminates **at the bridge**, not the daemon: a network hop drops
  the peer-credential identity the daemon's IPC relies on, so the bridge
  owns inbound auth.
- The bearer token is compared in **constant time** (`subtle`) to avoid a
  timing side-channel.
- Inbound request bodies are capped at **8 MiB** (the daemon separately
  enforces its 64 MiB frame cap).

### Accepted posture for the localhost default

The following are deliberate for a user-launched, loopback-default dev
tool (the same posture as Ollama's local server). They are **not**
hardened by default and matter only if you expose the bridge on a
network (which requires `--token`); put it behind a reverse proxy for
non-loopback use:

- **No per-connection / concurrency cap or SSE idle timeout.** A flood of
  slow streams could tie up resources. The daemon's admission queue still
  bounds *active* generations (`queue_full` → HTTP 429).
- **No CORS headers / CSRF protection.** A local malicious page could POST
  to `127.0.0.1` — inherent to any no-auth localhost service.
- **Daemon errors are surfaced verbatim** (e.g. socket path in a connect
  error) to aid local debugging; harden/scrub if exposing non-loopback.

## Compatibility notes

- inferd serves **one warm model**; the request `model` field is accepted
  and echoed, not validated.
- **Structured output** is supported: `response_format`
  `{"type":"json_schema","json_schema":{"schema":{…}}}` maps to the
  daemon's grammar-constrained decoding, so output conforms to your JSON
  Schema. `{"type":"json_object"}` / `{"type":"text"}` carry no schema, so
  they're unconstrained (best-effort JSON via the prompt only).
- **`tool_choice` is supported** for the three string forms (`"auto"`,
  `"required"`, `"none"`), which map to the daemon's constraint (ADR 0029)
  — `required` really cannot come back as prose. Two 400s: the
  named-function form `{"type":"function","function":{"name":…}}` is
  rejected rather than widened to `"required"` (widening would let the
  model call a *different* declared tool while you believed you had
  pinned one — send `"required"` with only that tool declared instead),
  and `tool_choice` alongside `response_format` is rejected because only
  one decoding constraint can be installed. Unlike `response_format`, an
  unrecognised `tool_choice` is an error, not a silent no-op.
- **Unsupported OpenAI params:** `n > 1` → 400; `logprobs`,
  `presence_penalty`, `frequency_penalty` are ignored. Image **and** audio
  input are supported (see Vision / Audio above). Audio *output*
  (`modalities: ["audio"]`) is not — inferd generates text.
- **Thinking** is not reachable through this bridge: it's an explicit
  opt-in on inferd's native wire (`RequestV2.thinking`), and the OpenAI
  Chat API has no standard field for it. Use `inferd-client` directly if
  you need reasoning traces. (Any thinking the model does emit is not
  surfaced as separate `content` here.)
- `finish_reason` maps from inferd's stop reason (`end_turn`→`stop`,
  `max_tokens`→`length`, `tool_use`→`tool_calls`).
- Daemon errors map to OpenAI-shaped errors with the right status
  (`queue_full`→429, `backend_unavailable`→503, `invalid_request`→400).

## Concurrency

Each HTTP request dials a fresh daemon connection; the daemon's admission
queue multiplexes. If the HTTP client disconnects, the daemon job is
cancelled (the connection drops).
