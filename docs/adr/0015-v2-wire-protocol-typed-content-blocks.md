# 0015. v2 wire protocol — typed content blocks, attachments, tools

- Status: accepted
- Date: 2026-05-20

## Context

ADR 0008 froze v1 of the wire protocol: `Message { role, content:
String }`, NDJSON-over-IPC, single inference socket per platform,
no in-band version negotiation. v1 covers the text-only single-
turn case the v0.1.x cycle has been validating.

ADR 0013 commits inferd to being an LLM gateway: the daemon
owns model-specific shaping. That requires consumers to express
*semantic* intent on the wire (which messages, which roles,
which attachments, which tools), not engine-shaped intent
(literal `<|turn>...<turn|>` tokens, base64'd images in prompt
strings). v1's `String` content can't carry that — it can only
carry text.

ADR 0008 promised that breaking changes go to v2 on a *separate
socket path*. This ADR specifies what v2 looks like, so:

1. Middleware authors writing v0.1 text-only code today can
   plan their v2 migration against a known target.
2. The `Backend` trait redesign (a real piece of v0.2 work)
   has a precise contract to satisfy.
3. The second-backend adapter (planned: OpenAI-compat) has a
   shape to translate between.

This ADR is **design only**. No code lands with this ADR. The
v2 surface ships as part of v0.2 work; the ADR locks the
contract so that work doesn't drift.

## Decision

The v2 wire protocol takes its shape from the cross-API
convergence on Anthropic's `/v1/messages` envelope. It is
**not** the Anthropic API verbatim — we strip the HTTP layer,
strip the JSON-Schema-style content-block tags inferd doesn't
need, and add a few inferd-specific fields (request id,
backend hint). But the *shape* is recognisable.

### Endpoints

v2 lives on a **separate socket / pipe / TCP port** from v1, per
ADR 0008. Operators choose to expose v2 via daemon CLI flags;
the daemon can serve v1 and v2 simultaneously.

| Platform | v1 inference | v2 inference | v2 admin |
|---|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.sock` | `${XDG_RUNTIME_DIR}/inferd/infer.v2.sock` | shared with v1 admin |
| macOS | `${TMPDIR}/inferd/infer.sock` | `${TMPDIR}/inferd/infer.v2.sock` | shared with v1 admin |
| Windows | `\\.\pipe\inferd-infer` | `\\.\pipe\inferd-infer-v2` | shared |

The admin socket is **shared between v1 and v2** because admin
is about daemon lifecycle, not request shaping; lifecycle events
are version-agnostic.

### Frame envelopes

Same NDJSON framing as v1: one JSON object per line, `\n`
terminated, 64 MiB per-frame cap, frames decode strictly with
`deny_unknown_fields = false` so additive changes within v2 stay
forward-compatible.

#### v2 Request

```json
{
  "id": "req-001",
  "messages": [
    {
      "role": "system",
      "content": [{"type": "text", "text": "You are helpful."}]
    },
    {
      "role": "user",
      "content": [
        {"type": "text", "text": "What's in this image?"},
        {"type": "image", "attachment_id": "img-1"}
      ]
    }
  ],
  "attachments": [
    {
      "id": "img-1",
      "kind": "image",
      "mime": "image/jpeg",
      "bytes": "<base64>"
    }
  ],
  "tools": [
    {
      "name": "get_weather",
      "description": "Returns the current weather for a city.",
      "input_schema": {
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"]
      }
    }
  ],
  "temperature": 0.7,
  "max_tokens": 1024,
  "stream": true
}
```

Required: `id`, `messages` (≥1).
Optional: `attachments`, `tools`, sampling params, `stream`.

#### v2 ContentBlock variants

| `type` | Required fields | Notes |
|---|---|---|
| `text` | `text: String` | The simple case. |
| `image` | `attachment_id: String` | References an entry in top-level `attachments[]`. The block doesn't carry bytes — that lives in `attachments[]` so it can be sent once and referenced multiple times in a single request. |
| `audio` | `attachment_id: String` | Same shape as image. |
| `video` | `attachment_id: String` | Same shape; engine support is a separate concern. |
| `tool_use` | `tool_call_id: String, name: String, input: Value` | Appears in `assistant`-role messages on the *response* side; consumers don't typically construct these. The model wants to call a tool; the daemon emits this in a `Response::Frame`. The consumer then sends the next request with a `tool_result` block. |
| `tool_result` | `tool_call_id: String, content: Vec<ContentBlock>` | Consumer-constructed. The result of executing a tool. The nested `content` is typically a single `text` block. |

Unknown `type` values are ignored on parse (forward-
compatibility — if v2.x adds a new content type, v2.0 daemons
ignore it gracefully and emit `invalid_request` if the model
needs it).

#### v2 Attachment

```json
{
  "id": "img-1",
  "kind": "image" | "audio" | "video",
  "mime": "image/jpeg",
  "bytes": "<base64>"
}
```

Size limits at the wire layer:
- `bytes` field is base64 of raw bytes. Inflated form must
  fit within the 64 MiB per-frame cap. Practical limit per
  attachment: ~48 MiB raw (base64 inflates ~1.33×).
- `attachments[]` total per request: limited only by the
  per-frame cap.

Engine-specific limits (the unsloth Q4_K_XL we ship today
doesn't include vision; multimodal Gemma 4 builds will impose
their own resolution / sample-rate limits) are reported in
`Response::Error{code: invalid_request}` if exceeded.

#### v2 Tool definition

Mirrors Anthropic's tool-use shape — JSON Schema for the
input, free-form description, name unique per request.

#### v2 Response frames

Streaming output is `Response::Frame { id, content_block }`
where `content_block` is one of the variants above.

```json
{"type":"frame","id":"req-001","block":{"type":"text","delta":"Hello "}}
{"type":"frame","id":"req-001","block":{"type":"text","delta":"there"}}
{"type":"frame","id":"req-001","block":{"type":"tool_use","tool_call_id":"tc-1","name":"get_weather","input":{"city":"London"}}}
```

Note: text blocks stream incrementally via `delta`. Tool-use
blocks arrive complete (the daemon parses the model's
`<|tool_call>...<tool_call|>` sequence in full before emitting).

Terminal frame (success):

```json
{"type":"done","id":"req-001","stop_reason":"end_turn","usage":{"input_tokens":N,"output_tokens":M},"backend":"llamacpp"}
```

`stop_reason` v2 values: `end_turn`, `max_tokens`, `tool_use`,
`stop_sequence`. (v1's `end` corresponds to `end_turn`.) Tool-
use stop-reason means the model is waiting for a `tool_result`
content block in the next message; the consumer executes the
tool and sends a follow-up request continuing the conversation.

Terminal frame (failure):

```json
{"type":"error","id":"req-001","code":"invalid_request","message":"image attachment exceeds engine input size"}
```

`code` v2 values: same as v1 (`queue_full`, `backend_unavailable`,
`invalid_request`, `internal`) plus:
- `attachment_unsupported` — the active backend can't handle
  this attachment kind (e.g. asking a text-only adapter for
  image input).
- `tool_call_malformed` — the model emitted a tool-call
  sequence the daemon couldn't parse cleanly.

### What lives where

The daemon (per ADR 0013) is responsible for:

- Validating v2 requests against the schema.
- Resolving `attachment_id` references in content blocks against
  the top-level `attachments[]`.
- Applying the engine-specific chat template to the assembled
  prompt (e.g. wrapping turns in Gemma's `<|turn>...<turn|>`).
- Routing attachments through the engine's binary side-channel
  (llama.cpp's mtmd; cloud backends' attachment fields).
- Detecting and emitting tool-use blocks from the raw token
  stream.
- Parsing `tool_result` content blocks in the user's next
  request and routing the result text back into the model's
  context.

The consumer (middleware, CLI, agent) is responsible for:

- Acquiring binary blobs (image / audio / video).
- Validating wire-layer limits before send (size, MIME).
- Constructing `messages[]` semantically — placing image
  blocks where the user referenced them, not as opaque text.
- Defining tools with JSON Schema input descriptors.
- Executing tools when the daemon emits a tool-use block, and
  sending the next request with a corresponding `tool_result`.
- Managing session memory — what stays across turns.

### Backend trait implications

The v0.2 `Backend` trait grows to absorb the gateway
responsibilities. Sketch:

```rust
trait Backend {
    fn name(&self) -> &str;
    fn ready(&self) -> bool;
    fn supports(&self) -> BackendCapabilities;  // text + image + audio + tools

    async fn generate_v2(
        &self,
        resolved: ResolvedRequest,    // post-validation, attachments resolved
        attachments: &[Attachment],   // raw bytes side-channel
    ) -> Result<TokenStreamV2, GenerateError>;
}
```

`ResolvedRequest` carries the typed content blocks; attachments
are passed alongside as a slice so the backend can route them
through whatever side-channel its engine wants (mtmd for
llamacpp; multipart fields for cloud HTTP backends).

The `TokenStreamV2` yields typed events:
`Text(delta)`, `ToolUse { id, name, input }`, `Done { ... }`.

### Config additions

Multimodal Gemma 4 needs a separate projector blob. The config
schema gains an optional `model.mmproj` field:

```json
{
  "model": {
    "name": "gemma-4-e4b-multimodal",
    "sha256": "...",
    "size_bytes": 5000000000,
    "source_url": "...",
    "license": "apache-2.0",
    "mmproj": {
      "sha256": "...",
      "size_bytes": 800000000,
      "source_url": "..."
    }
  }
}
```

If present, the daemon's startup fetch path pulls both blobs
(matching SHAs) into the CAS store and hands both paths to the
backend on init.

## Consequences

**Why this is the right shape:**

- **Anthropic-shape parity.** Middleware authors with experience
  writing against Anthropic / OpenAI / Bedrock recognise the
  envelope immediately. The cognitive cost of supporting both a
  cloud API and inferd in one middleware is small.
- **Maps cleanly to llama.cpp's mtmd interface.** Raw bytes
  through a side-channel matches mtmd's actual API; we don't
  fight the engine.
- **Forward-compatible additively.** New content-block types
  (e.g. a future `document` block, or per-content provenance)
  fit by adding new `type` values that older parsers ignore.
- **v1 remains untouched.** No retroactive breakage. Migration
  is opt-in via the v2 socket.
- **Tools are first-class.** The lifecycle is on the wire; the
  consumer doesn't have to grep token streams for
  `<|tool_call>` sequences.

**What this costs:**

- Real implementation work in the daemon (chat templating
  per-engine, attachment routing, tool-call parsing).
- The `Backend` trait gets significantly larger; existing
  `mock` and `llamacpp` adapters need a v2 generate path
  alongside their v1 path.
- Middleware that wants v0.2 features writes more JSON. v1 is
  one short string per message; v2 is a typed array per
  message plus a top-level attachments table. Operators
  should keep v1 enabled by default for the operationally-
  simpler text-only case.
- The wire schema expands the surface area v0.2 commits to
  freezing. Once v2 ships, the typed-content-block format is
  as immutable as v1 is.

**What this explicitly does not change:**

- ADR 0006 — daemon stays HTTP-free.
- ADR 0007 — no in-daemon retry, no mid-stream failover.
  Tool-call lifecycle is contract, not retry.
- ADR 0008 — v1 still frozen on its own socket.
- ADR 0011 — CAS store; multimodal projector blob fits
  naturally as another blob with its own SHA + manifest.
- ADR 0012 — one warm model per process; multimodal Gemma is
  still one model (just with projector layers).
- ADR 0013 — gateway framing is exactly what this ADR
  concretises.
- ADR 0014 — CLI uses v2 the same way external consumers do
  (no special path).

## Alternatives considered

- **Extend v1 in-band with optional fields for typed content.**
  Rejected. ADR 0008 specifically said breaking changes go to
  v2 on a separate socket. Typed content blocks would either
  break v1 parsers (if `content` becomes typed) or require a
  parallel `content_v2` field that pollutes the v1 schema.
- **Anthropic-compatible v2 verbatim** (HTTP, SSE, etc.).
  Rejected — that violates ADR 0006, and the framing /
  transport differences are exactly what makes inferd useful
  as a local IPC gateway. Borrow the *content shape*, not the
  transport.
- **Defer v2 protocol design until the second-backend work
  starts.** Rejected. Middleware authors writing v0.1 today
  benefit from knowing the v2 target. Locking the design
  before code costs nothing and prevents drift.
- **Embed binary attachments inline in the content block (no
  attachment_id indirection).** Rejected. The indirection lets
  middleware reference the same attachment from multiple
  positions in the message without duplicating bytes. Matches
  Anthropic's shape; minor cost.

## References

- ADR 0006 — lean-core posture (unchanged).
- ADR 0008 — v1 frozen, v2 on separate socket (the rule this
  ADR follows).
- ADR 0011 — shared CAS model store (mmproj fits naturally).
- ADR 0012 — one warm model per inferd process (unchanged).
- ADR 0013 — gateway framing (this ADR concretises it).
- ADR 0014 — CLI as reference middleware (will adopt v2 at
  the same time external consumers can).
- Gemma 4 prompt format upstream docs — the engine-side
  formatting the daemon will apply on behalf of consumers.
- Anthropic `/v1/messages` API — the shape this ADR borrows.
