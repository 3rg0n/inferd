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

- `POST /v1/chat/completions` — streaming (SSE) and non-streaming.
- `POST /v1/embeddings` — `float` and `base64` encodings (the OpenAI SDK
  defaults to `base64`).
- `GET /v1/models` — advertises the single warm model.
- `GET /health` — liveness.

## Run

```sh
# The inferd daemon must be running locally first.
inferd-http                       # binds 127.0.0.1:8080, no auth
inferd-http --listen 127.0.0.1:9000
inferd-http --model-name my-model # what /v1/models advertises + echoes
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

## Compatibility notes

- inferd serves **one warm model**; the request `model` field is accepted
  and echoed, not validated.
- **Unsupported OpenAI params:** `n > 1` → 400; `logprobs`,
  `presence_penalty`, `frequency_penalty` are ignored; multimodal message
  content (image/audio) is not accepted in this version (text only).
- **Thinking traces** from the model are not surfaced (no OpenAI public
  channel); the answer text streams as normal `content`.
- `finish_reason` maps from inferd's stop reason (`end_turn`→`stop`,
  `max_tokens`→`length`, `tool_use`→`tool_calls`).
- Daemon errors map to OpenAI-shaped errors with the right status
  (`queue_full`→429, `backend_unavailable`→503, `invalid_request`→400).

## Concurrency

Each HTTP request dials a fresh daemon connection; the daemon's admission
queue multiplexes. If the HTTP client disconnects, the daemon job is
cancelled (the connection drops).
