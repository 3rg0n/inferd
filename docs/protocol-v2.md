# inferd wire protocol — v2 generation + embeddings + rerank (normative spec)

> **Status:** normative for inferd **v0.4.0 and later** (current: v0.6.1). This document is the
> contract an implementer writes middleware against. Where this document
> and the `inferd-proto` source disagree, the source
> (`crates/inferd-proto/`) wins and this document is the bug — but CI
> guards the message-body schemas against drift (see
> [§10](#10-conformance-vectors)). Framing and sockets are specified by
> [ADR 0021](adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)
> (generation), [ADR 0017](adr/0017-embeddings-on-a-third-socket.md)
> (embeddings), [ADR 0027](adr/0027-reranking-on-a-fourth-socket.md)
> (rerank), and [ADR 0009](adr/0009-pre-m1-open-questions-resolved.md)
> (admin). `docs/protocol-v1.md` describes the **removed** v1 surface and
> is historical only.

This spec is written to be consumed whole — by a human or by a model
asked to "write a client/middleware for inferd." It is self-contained:
every type, every byte, and the enumerated error set are inline. The
key words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY**
are used per [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119).

---

## 1. Surfaces, sockets, and discovery

inferd exposes four IPC surfaces, each on its own endpoint. A daemon
binds an inference surface **only when** the active backend advertises
that capability, and binds the inference socket **only after** the
backend reports `ready` (so a successful connect to an inference socket
is itself the readiness signal). The admin socket is bound early,
during bring-up.

| Surface    | Framing                          | Unix socket name      | Windows pipe                   |
|------------|----------------------------------|-----------------------|--------------------------------|
| generation | length-prefixed, type-tagged     | `inferd.sock`         | `\\.\pipe\inferd`              |
| embeddings | NDJSON (newline-delimited JSON)  | `infer.embed.sock`    | `\\.\pipe\inferd-infer-embed`  |
| rerank     | NDJSON                           | `infer.rerank.sock`   | `\\.\pipe\inferd-infer-rerank` |
| admin      | NDJSON                           | `admin.sock`          | `\\.\pipe\inferd-admin`        |

Which sockets exist tells you what the warm model can do: a
generation-only model binds `inferd.sock` and nothing else, an
embedding-only model binds only `infer.embed.sock`, a cross-encoder
reranker binds only `infer.rerank.sock`. One inferd process holds one
warm model ([ADR 0012](adr/0012-one-warm-model-per-inferd-process.md)),
so a deployment wanting generation *and* embeddings *and* rerank runs
three inferd processes.

Socket presence is the ground truth, but a consumer can also read the
capability set *before* dialling, off the admin socket's `capabilities`
frame — one such frame per registered backend:

```json
{"status":"capabilities","backend":"bge-reranker-v2-m3","v2":false,"embed":false,"rerank":true, ...}
```

`embed` and `rerank` are independent: a bi-encoder embedding model
reports `embed: true, rerank: false`, because rerank needs a
classification head and a RANK-pooling context it cannot share. `rerank`
is omitted when false — as it is by any daemon before v0.6.2 — so an
absent key means *not supported*, never *unknown*.

A connection is a **stream** (`SOCK_STREAM` UDS on Unix, a named-pipe
byte stream on Windows). The same connection MAY carry multiple
sequential requests; responses for a request arrive before the next
request's first response. There is no request multiplexing on a single
connection — one in-flight request at a time per connection.

The daemon binds **no inbound network listener** — it is reachable only
over the local UDS / named pipe ([ADR 0022](adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md)).
Anything that needs to reach inferd over a network port goes through the
separate `inferd-http` bridge ([ADR 0020](adr/0020-inferd-http-bridge-is-a-separate-process.md)),
not the daemon. The `inferd-http` bridge binary is bundled with each release tarball
alongside `inferd-daemon` and `inferdctl`.

### 1.1 Unix socket path resolution

On Unix the socket directory is resolved in this order, first hit wins:

1. `$XDG_RUNTIME_DIR/inferd/<name>` (Linux, when systemd-logind set it)
2. `$HOME/.inferd/run/<name>` (sessions without logind)
3. `/tmp/inferd/<name>` (last resort)

On macOS the directory is `${TMPDIR}/inferd/<name>`. On Windows the
named pipes above are absolute.

### 1.2 Transport security (informative)

- UDS is mode `0660`, group `inferd-users`; the admin socket is `0600`.
  Peer credentials (UID on Unix, SID on Windows) are enforced — the OS
  attests who connected, so there is no in-band auth handshake to send.
  A client connects and immediately sends its first request frame.
- The daemon exposes no network transport, so there is no API-key or TLS
  story at the daemon layer; that lives in the `inferd-http` bridge
  (ADR 0020) for callers that need network access.

---

## 2. Generation framing (length-prefixed, type-tagged)

Every frame on the **generation** socket is:

```
┌──────────────────┬──────────────┬────────────────────────┐
│  payload_len      │  frame_type  │  payload                │
│  uvarint (LEB128) │  1 byte      │  exactly payload_len B  │
└──────────────────┴──────────────┴────────────────────────┘
```

- **`payload_len`** — unsigned LEB128 varint. Counts the bytes of
  `payload` **only**; it does **NOT** include the `frame_type` byte. A
  reader MUST cap the varint at **5 bytes** (64 MiB fits in 27 bits =
  4 groups; 5 is the hard stop). A varint that does not terminate within
  5 bytes is a malformed frame.
- **`frame_type`** — exactly one byte:
  - `0x01` = **JSON** — payload is UTF-8 JSON (a control frame: request,
    response, or blob descriptor).
  - `0x02` = **BLOB** — payload is raw bytes (decoded media), correlated
    to an attachment by a preceding `attachment_blob` descriptor.
  - Any other byte MUST be treated as a malformed frame; the reader
    closes the connection.
- **`payload`** — exactly `payload_len` bytes. No trailing delimiter.

**Frame cap (THREAT_MODEL F-5):** `payload_len` MUST be `≤ 64 MiB`
(`67108864`). A reader MUST check this against the decoded varint
**before reading any payload byte** and MUST NOT allocate a payload
buffer larger than the cap. On overflow the reader closes the connection
— it MUST NOT attempt to resync, because the byte stream is no longer
trustworthy.

**Clean close:** EOF *before the first byte of `payload_len`* is a clean
between-frames close. EOF anywhere after that (mid-varint, mid-type,
mid-payload) is a malformed/truncated frame.

Writers MUST flush per frame (or use an unbuffered writer) — consumers
rely on per-frame visibility for streaming.

### 2.1 LEB128 reference (the only non-JSON codec you must implement)

```
write_uvarint(n):
    loop:
        byte = n & 0x7F
        n  >>= 7
        if n != 0: byte |= 0x80     # continuation bit
        emit byte
        if n == 0: stop

read_uvarint(stream):              # MUST stop after 5 bytes
    value = 0; shift = 0
    for i in 0..5:
        b = read 1 byte            # EOF at i==0 → clean close
        value |= (b & 0x7F) << shift
        if (b & 0x80) == 0: return value
        shift += 7
    error: varint too long
```

---

## 3. Generation request

### 3.1 Connection lifecycle

```
client                              daemon
  │ connect inferd.sock  ──────────►│   (connect succeeds ⇒ backend ready)
  │ JSON frame: RequestV2 ─────────►│
  │ [per attachment, in order:]     │
  │   JSON frame: BlobDescriptor ──►│
  │   BLOB frame: raw bytes ───────►│
  │                                 │
  │ ◄──── JSON frame: ResponseV2 {type:"frame"}   (0..N, streamed)
  │ ◄──── JSON frame: ResponseV2 {type:"done" | "error"}  (exactly 1, terminal)
```

A request is: **one** JSON frame carrying `RequestV2`, then — for each
attachment that carries bytes, **in the order they appear in
`attachments[]`** — a JSON `BlobDescriptor` frame immediately followed by
its `0x02` BLOB frame. Text-only requests send just the one JSON frame.

The daemon streams zero or more `frame` responses, then exactly one
terminal frame (`done` **or** `error`). After the terminal frame the
connection MAY be reused for the next request.

**Connection reuse:** after reading a terminal frame, a client MAY send
another `RequestV2` (its own JSON frame + any BLOBs) on the same
connection — the daemon loops back to reading the next request frame.
There is no separator or reset between requests beyond the terminal
frame of the previous one; the next `payload_len` varint begins
immediately. Requests are strictly sequential — do not pipeline a second
request before the first has terminated. A client that does not reuse the
connection simply closes it after the terminal frame.

**`stream` field:** `stream` controls only whether intermediate `frame`
responses are emitted. With `stream: false` the daemon withholds the
incremental `text`/`thinking` deltas and emits the terminal `done`
(or `error`) frame alone; a `tool_use` block, being a complete unit, is
still delivered as a `frame`. The terminal frame's `usage` and
`stop_reason` are identical either way. Default (field omitted) is
streaming.

**Cancellation:** closing the connection cancels the in-flight job. The
daemon does not retry or fail over (ADR 0007) — retry is the caller's
responsibility.

### 3.2 `RequestV2` (JSON)

| Field          | JSON key        | Type                     | Required | Notes |
|----------------|-----------------|--------------------------|----------|-------|
| wire_version   | `wire_version`  | uint32                   | **yes**  | MUST be `1` for v0.4. A frame omitting it deserialises as `0` and is rejected. |
| id             | `id`            | string                   | no\*     | Correlation id, echoed on every response frame. Omitted ⇒ empty string. \*Strongly recommended. |
| messages       | `messages`      | array of `MessageV2`     | **yes**  | MUST be non-empty. |
| attachments    | `attachments`   | array of `Attachment`    | no       | Metadata only — bytes ride in BLOB frames. Omit when text-only. |
| tools          | `tools`         | array of `Tool`          | no       | Tool definitions in scope for this request. |
| tool_choice    | `tool_choice`   | string                   | no       | Whether the model may / must / must not call a tool: `"auto"`, `"required"`, `"none"` (§3.2b). Requires a non-empty `tools`. Unlike `response_format`, a value the daemon cannot honour is an **error**, not a silent no-op. |
| temperature    | `temperature`   | float                    | no       | Daemon applies the backend default if absent. |
| top_p          | `top_p`         | float                    | no       | "" |
| top_k          | `top_k`         | uint32                   | no       | "" |
| max_tokens     | `max_tokens`    | uint32                   | no       | "" |
| stream         | `stream`        | bool                     | no       | Defaults to streaming. |
| response_format | `response_format` | `ResponseFormat` object | no       | Structured output constraint (e.g. JSON Schema). Backends that don't support it ignore this field. |
| thinking       | `thinking`      | bool                     | no       | Request reasoning mode. `true` asks the model to produce an internal reasoning trace, returned as `thinking` response blocks (§4.1) separate from user-visible `text`. Omitted/`false` = no thinking (default). The daemon shapes activation per engine (Gemma 4: injects `<|think|>` into the system turn); backends without reasoning support ignore it. |

A parser **MUST ignore unknown top-level fields** (forward-compat).

### 3.2a `ResponseFormat` (tagged by `type`, snake_case)

Specifies a structured output format constraint on the model's generation.
The daemon translates the semantic format to engine-specific constraints
(e.g. GBNF grammar for llamacpp).

```
{ "type": "json_schema", "schema": <JSON Schema object> }
```

- `type` — exactly `"json_schema"` (enum; future values may be added).
- `schema` — a JSON Schema object. The model output will be constrained
  to match this schema. Backends that don't support structured output
  (or don't support JSON Schema specifically) ignore this field and return
  unconstrained output; there is **no error** — the request succeeds as if
  `response_format` were absent.

### 3.2b `tool_choice` (bare string)

Constrains tool use for this request. Three values:

| Value        | Meaning |
|--------------|---------|
| `"auto"`     | The model decides. Behaviourally the same as omitting the field, except the daemon may additionally constrain the *shape* of a call the model chooses to make. |
| `"required"` | The model MUST emit at least one tool call. On a backend that enforces this, no path through sampling produces a bare text answer. |
| `"none"`     | The model MUST NOT call a tool. Declarations still reach the prompt, so the rendered context is unchanged. |

Rules:

- **`tools` MUST be non-empty.** `tool_choice` constrains that table; sent
  without it, the request is rejected with `invalid_request`.
- **`response_format` and `tool_choice` are mutually exclusive.** Both
  constrain decoding and only one constraint can be installed, so a
  request carrying both is rejected with `invalid_request` rather than
  having one of them silently dropped (ADR 0029).
- **This is a constraint, not a hint.** A backend that cannot enforce the
  requested mode rejects the request with `invalid_request`; it does not
  accept it and try. This is the deliberate difference from
  `response_format`, which degrades to unconstrained output. A caller
  that sets `required` and receives a `done` frame with no `tool_use`
  block has hit a bug, not a documented degradation.
- **An unrecognised value parses but is rejected.** A newer client's
  request deserialises (forward-compat) and the daemon then answers
  `invalid_request` rather than guessing which mode was meant.
- **Naming a specific tool is not expressible.** There is no
  `{"type":"function","function":{"name":…}}` form. To pin one tool, send
  `"required"` with only that tool declared. Bridges MUST reject the
  named form rather than widening it to `"required"`, which would let the
  model call a *different* declared tool while the caller believed it had
  pinned one.
- **Scope: names, not argument schemas.** Enforcement pins the call
  syntax and constrains the tool name to the declared table; argument
  *values* are not constrained by each tool's `input_schema`. Callers
  still validate arguments.

### 3.3 `MessageV2`

```
{ "role": "system" | "user" | "assistant", "content": [ ContentBlock, ... ] }
```

`content` MUST be non-empty. Roles are exactly `system`, `user`,
`assistant` (lowercase). There is no `tool` role — a tool call is an
`assistant` message containing a `tool_use` block, and a tool result is a
`user` message containing a `tool_result` block (Anthropic shape).

### 3.4 `ContentBlock` (tagged by `type`, snake_case)

| `type`        | Fields                                            | Direction |
|---------------|---------------------------------------------------|-----------|
| `text`        | `text`: string                                    | request + response context |
| `image`       | `attachment_id`: string                           | request |
| `audio`       | `attachment_id`: string                           | request |
| `video`       | `attachment_id`: string (reserved; daemons reject with `attachment_unsupported`) | request |
| `tool_use`    | `tool_call_id`: string, `name`: string, `input`: JSON object | replayed assistant turns |
| `tool_result` | `tool_call_id`: string, `content`: array of `ContentBlock` | request |

An `attachment_id` on an `image`/`audio`/`video` block MUST match
exactly one `Attachment.id` of the corresponding kind in the request's
`attachments[]`. Unknown `type` values: a forward-compatible parser
decodes them as an "unknown" block and ignores them rather than erroring
at parse time; the daemon rejects a request only if it *needs* the
unknown block to proceed.

### 3.5 `Attachment` (tagged by `kind`, lowercase) — metadata only

Raw bytes do **NOT** appear in this JSON. They travel in a BLOB frame
(see §3.7). The consumer decodes media to raw bytes before sending — the
daemon links no image/audio codec (ADR 0016).

| `kind`  | Fields                                          | Bytes in the BLOB frame |
|---------|-------------------------------------------------|-------------------------|
| `image` | `id`: string, `width`: uint32, `height`: uint32 | `width*height*3` interleaved RGB octets (no alpha) |
| `audio` | `id`: string, `sample_rate`: uint32 (Hz) — MUST equal the backend's advertised `audio_sample_rate` | little-endian float32 PCM samples |
| `video` | `id`: string                                    | reserved; format TBD |

`id` MUST be unique within a request.

#### Image resolution / OCR fidelity (operator-controlled)

The daemon owns image preprocessing (ADR 0013): for a dynamic-resolution
vision model (the default Gemma 4 is one), the projector downscales /
tiles the decoded RGB toward a token budget before encoding. Dense
paragraph text survives this, but **small or sparsely-spaced text — OCR
of fine print, or title lines with dotted leaders — can drop below
legibility at the downscale**, even when the consumer sends ample
resolution (issue #42). The loss is in daemon-side encoding, not the
wire: sending a larger image does not by itself help once it exceeds the
projector's budget.

There is **no per-request resolution knob** on the wire, by design — image
shaping is the daemon's job, not the consumer's, and the budget is a
model/context property fixed at load time (re-tuning it per request would
mean re-initialising the projector). Instead it's an **operator config**:
the llamacpp backend entry accepts `mmproj_image_max_tokens` (maps to
libmtmd's `image_max_tokens`). Raising it reduces downscaling so sparse /
small text survives, at the cost of more image tokens and slower encode;
`null`/omitted (the default) reads the model's metadata default and is
unchanged behaviour. A consumer that needs higher OCR fidelity than the
deployment's configured budget should pre-segment or upscale the region
of interest before sending, or ask the operator to raise the budget.

#### Audio `sample_rate` MUST match the backend's advertised rate

libmtmd's audio entry point takes no rate argument — it consumes the
samples at whatever rate the loaded encoder was trained for. PCM at any
other rate is therefore **not a detectable error**: the audio is
effectively time-scaled and the model answers plausibly but wrongly.

So `sample_rate` is a contract, not a hint. The daemon does not resample
(the consumer decodes, so it owns rate conversion — ADR 0016), and it
**rejects** an audio attachment whose `sample_rate` differs from the
active backend's required rate with `invalid_request`, naming both rates.

Discover the required rate from the admin socket's `capabilities` frame:

```json
{"status":"capabilities","backend":"llamacpp","audio":true,"audio_sample_rate":16000, ...}
```

`audio_sample_rate` is omitted when the backend ingests no audio or
reports no rate; when it is absent, no rate check is applied. Consumers
MUST resample to the advertised value rather than hardcoding 16000.
`inferdctl status` relays it (as it does every other field on the frame),
so the rate is readable without writing an admin-socket client:
`inferdctl status | jq -r 'select(.audio) | .audio_sample_rate'`.

`inferd-http` is the reference implementation of that obligation (ADR
0025): it decodes the client's wav/mp3, downmixes to mono, and resamples
to the rate it read off this frame — re-reading it per audio request, so a
daemon restart onto a different mmproj cannot desynchronise the two.

### 3.6 `Tool`

```
{ "name": string, "description": string, "input_schema": <JSON Schema object> }
```

`name` MUST be unique within a request. The daemon does **not** enforce
`input_schema` against the model's emitted arguments — that is the
consumer's responsibility when the `tool_use` result comes back.

### 3.7 `BlobDescriptor` (JSON frame preceding each BLOB)

```
{ "type": "attachment_blob", "attachment_id": string, "len": uint64 }
```

`attachment_id` correlates the following `0x02` BLOB frame to an
`Attachment.id` in the already-sent `RequestV2`. `len` is the byte length
of that BLOB and SHOULD equal the BLOB's `payload_len`; a reader uses it
as a sanity check.

**Per-request attachment bounds (THREAT_MODEL F-1).** The frame cap in §2
bounds one *frame*; the attachment table bounds how many frames one
request may demand, so it needs its own limits. A request MUST NOT
declare more than **32** attachments, and the sum of all its BLOB lengths
MUST NOT exceed **128 MiB**. A reader MUST reject an over-count request
before reading any BLOB, and MUST charge the byte budget against this
descriptor's `len` **before** reading the payload it describes — so an
over-budget request costs the reader no allocation. Over-count is
`invalid_request`; over-budget is `frame_too_large`. These are separate
from, and stricter than, `32 × 64 MiB`.

---

## 4. Generation response

Each response is a `0x01` JSON frame holding a `ResponseV2`, tagged by
`type` (snake_case). The daemon emits 0..N `frame`s then exactly one
terminal frame. A response frame on the generation socket is **always**
type `0x01`; a `0x02` frame on the response stream is a protocol error.

### 4.1 `frame` (streaming, non-terminal)

```
{ "id": string, "type": "frame", "block": ResponseBlock }
```

`ResponseBlock` is tagged by `type`:

| `type`     | Fields                                                  | Meaning |
|------------|---------------------------------------------------------|---------|
| `text`     | `delta`: string                                         | Incremental user-visible text. Concatenate deltas for the full answer. |
| `thinking` | `delta`: string                                         | Incremental reasoning trace, separated so middleware can show/hide/log it independently. |
| `tool_use` | `tool_call_id`: string, `name`: string, `input`: JSON   | A **complete** tool-call request (not streamed). Usually followed by a `done` with `stop_reason: "tool_use"`. |

### 4.2 `done` (terminal, success)

```
{
  "id": string,
  "type": "done",
  "usage": { "input_tokens": uint32, "output_tokens": uint32 },
  "stop_reason": "end_turn" | "max_tokens" | "tool_use" | "stop_sequence" | "cancelled" | "error",
  "backend": string
}
```

`backend` is the serving adapter's name (e.g. `"llamacpp"`). It is
**diagnostic only** — consumers MUST NOT branch on it (ADR 0007).

### 4.3 `error` (terminal, failure)

```
{ "id": string, "type": "error", "code": ErrorCodeV2, "message": string }
```

`ErrorCodeV2` is one of the following (snake_case). This set is closed
for v2.0; a consumer SHOULD treat an unrecognised code as a generic
failure.

| code                       | Meaning |
|----------------------------|---------|
| `queue_full`               | Admission queue full at submit time (non-blocking; caller may retry later). |
| `backend_unavailable`      | Selected backend errored before/during generation. |
| `invalid_request`          | Failed validation (bad shape, dangling `attachment_id`, empty messages/content, …). |
| `frame_too_large`          | A frame exceeded the 64 MiB cap. |
| `internal`                 | Daemon-side bug or unexpected condition. |
| `attachment_unsupported`   | Backend can't handle the attachment kind/MIME (e.g. video today). |
| `tool_call_malformed`      | Model emitted a tool-call sequence the daemon couldn't parse. |
| `wire_version_unsupported` | Request's `wire_version` is not one this daemon speaks (see §6). `message` names both requested and supported versions. |

---

## 5. Worked example — one text request, on the wire

A minimal text request (`max_tokens: 4`). Bytes shown as hex; `··`
separates the three frame parts for readability only.

**client → daemon** (one JSON frame):

```
payload (UTF-8 JSON):
  {"wire_version":1,"id":"r1","messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}],"max_tokens":4}

on the wire:
  6F ·· 01 ·· 7B 22 77 69 72 65 5F 76 65 72 73 69 6F 6E 22 3A 31 ...
  └┬┘    └┬┘  └──────────────── payload, 0x6F = 111 bytes ─────────────►
  len=111 type=JSON
```

(`0x6F` = 111; the JSON above is 111 bytes. `payload_len` does not count
the `01` type byte.)

**daemon → client** (streamed JSON frames, payloads shown):

```
{"id":"r1","type":"frame","block":{"type":"text","delta":"Hello"}}
{"id":"r1","type":"frame","block":{"type":"text","delta":" there"}}
{"id":"r1","type":"done","usage":{"input_tokens":12,"output_tokens":2},"stop_reason":"end_turn","backend":"llamacpp"}
```

For an **image** request the client would, after the `RequestV2` frame,
send: a JSON frame `{"type":"attachment_blob","attachment_id":"img1","len":196608}`
then a `0x02` BLOB frame of 196608 raw RGB bytes (256×256×3).

---

## 6. The `wire_version` handshake

There is **no** separate negotiation round-trip. The client stamps
`wire_version` (currently `1`) on every `RequestV2`. The daemon checks it
against the version it speaks **before** dispatching:

- Match → normal processing.
- Mismatch (including `0`, i.e. the field was omitted / a pre-v0.4
  client) → the daemon emits **exactly one** terminal `error` frame with
  `code: "wire_version_unsupported"` and **closes the connection**. It
  MUST NOT parse the request body or hang.

A v0.3 client does **not** interoperate with a v0.4 daemon — the framing
itself changed. Upgrade both together. Within v2, changes are
backwards-additive only (new optional fields, ignored by older peers); a
breaking change bumps `wire_version` so the mismatch fails loudly rather
than corrupting the stream (ADR 0021).

---

## 7. Embeddings surface (NDJSON)

The embeddings socket uses **newline-delimited JSON**, not the
length-prefixed framing: each frame is one JSON object terminated by
`\n`. The 64 MiB cap is enforced on the line length. One request → one
terminal response (embeddings are not streamed); the connection MAY be
reused.

**request** (`EmbedRequest`):

```
{ "id": string, "input": [string, ...], "dimensions"?: uint32, "task"?: EmbedTask }
```

- `input` MUST be non-empty and each entry MUST be non-empty.
  `embeddings[i]` corresponds to `input[i]`.
- `dimensions` — Matryoshka truncation length. EmbeddingGemma supports
  `768 | 512 | 256 | 128`; the backend validates and rejects with
  `invalid_request` otherwise. Omit for the model default.
- `task` — task-prefix hint, one of: `retrieval_query`,
  `retrieval_document`, `similarity`, `classification`, `clustering`,
  `question_answering`, `fact_verification`, `code_retrieval_query`. An
  unknown task value is rejected with `invalid_request`. The daemon
  applies the engine-specific prefix on the consumer's behalf (ADR 0013).

**response** (`EmbedResponse`, tagged by `type`):

```
// success
{ "type": "embeddings", "id": string, "embeddings": [[f32, ...], ...],
  "dimensions": uint32, "model": string,
  "usage": { "input_tokens": uint32 }, "backend": string }

// failure
{ "type": "error", "id": string, "code": EmbedErrorCode, "message": string }
```

`EmbedErrorCode` ∈ { `queue_full`, `backend_unavailable`,
`invalid_request`, `frame_too_large`, `internal`, `embed_unsupported` }.
`embed_unsupported` is a fail-safe — the embed socket should not have
been bound for a generation-only backend.

---

## 8. Rerank surface (NDJSON)

Rerank is **cross-encoder** reordering ([ADR 0027](adr/0027-reranking-on-a-fourth-socket.md)):
query and document are scored *together*, one model forward pass per
document. That is the whole reason it is a separate surface from embed
rather than a flag on it — a bi-encoder embeds each text once and
independently, so vectors can be precomputed and cached, while a
cross-encoder can precompute nothing. Rerank therefore sits **downstream
of retrieval**, over a candidate set embed already narrowed:

```
embed → vector search → top-50 → rerank → top-5 → generation
```

Framing matches embed: **newline-delimited JSON**, one JSON object per
`\n`-terminated line, 64 MiB cap enforced on the line length. One request
→ one terminal response (an ordering is not streamed — a partial ordering
isn't usable); the connection MAY be reused.

**request** (`RerankRequest`):

```
{ "id": string, "query": string, "documents": [string, ...], "top_n"?: uint32 }
```

- `query` MUST be non-empty.
- `documents` MUST be non-empty, each entry MUST be non-empty, and
  `results[].index` refers back into this array. The daemon never echoes
  document text — the caller already has it, and returning it would
  multiply the response size for nothing.
- `documents` MUST NOT exceed **256** entries, and `query` +
  `documents` MUST NOT exceed **8 MiB** of text in total. Both are
  rejected with `invalid_request`. These bound *work*, not bytes: the
  frame cap alone would let one cheap in-cap frame describe hundreds of
  thousands of forward passes while holding the shared admission permit
  (the THREAT_MODEL F-1 amplification class). Clients SHOULD pre-trim
  client-side — `inferd-client` re-exports both constants for that.
- `top_n` — return only the `n` highest-scoring results. Omitted returns
  all of them. `0` is rejected (`invalid_request`): an empty result set
  is never what a caller wants, and returning one silently would be
  indistinguishable from a backend that scored nothing. A `top_n` larger
  than `documents.len()` is **not** an error — it returns everything, so
  a caller whose candidate set shrank need not clamp.

**response** (`RerankResponse`, tagged by `type`):

```
// success
{ "type": "rerank", "id": string,
  "results": [{ "index": uint32, "score": f32 }, ...],
  "model": string, "usage": { "input_tokens": uint32 }, "backend": string }

// failure
{ "type": "error", "id": string, "code": RerankErrorCode, "message": string }
```

- `results` arrives **already sorted by `score` descending and already
  truncated to `top_n`**. The daemon owns both, because the score scale is
  model-specific and re-deriving the ordering in each consumer invites
  drift. Ties preserve input order (the sort is stable).
- `score` is the **raw** model output — not normalised, not squashed into
  `0..1`. It is **ordinal within one response only**: never compare scores
  across requests, across models, or against a fixed threshold. Negative
  values are ordinary (most cross-encoders emit logits). A synthetic
  `0..1` range would make incomparable numbers look comparable, which is
  the more expensive failure.
- `usage.input_tokens` is summed across every query/document pair, so it
  is roughly `documents.len() ×` the per-pair length — rerank's cost
  profile, made visible.

`RerankErrorCode` ∈ { `queue_full`, `backend_unavailable`,
`invalid_request`, `frame_too_large`, `internal`, `rerank_unsupported` }.
`rerank_unsupported` is a fail-safe like `embed_unsupported`: the rerank
socket should not have been bound for a backend that cannot serve it.
`queue_full` is shared with every other surface — one admission slot is
one slot regardless of which socket asked for it.

---

## 9. Reference implementations

- **Rust:** `inferd-client` (`crates/inferd-client/`) —
  `ClientV2` + `EmbedClient` + `RerankClient`. The wire types are
  `inferd-proto` (`crates/inferd-proto/src/v2/`, `.../embed/`, and
  `.../rerank/`), which this document mirrors.
- **Go:** `clients/go/` — the canonical non-Rust reference. The frame
  codec is `clients/go/client_v2.go`; the message types are
  `clients/go/protocol_v2.go`. ~440 lines total: a complete, idiomatic
  implementation of everything above.

When in doubt, read the Go codec — it is small and exercised by CI
against the live daemon.

---

## 10. Conformance vectors

A future revision will ship `docs/protocol-vectors.json`: `input → exact
wire bytes` pairs an implementer can assert their codec against,
generated from `inferd-proto` so they never drift. Until then, the Go
client's round-trip tests (`clients/go/client_v2_test.go`) are the
executable conformance reference. The message-body schemas in this
document are kept honest against the Rust types by the
`inferd-proto` test suite; the framing is covered by the
`frame.rs` unit tests (`lp_tests`).

---

## 11. Invariants a client author must respect

1. Stamp `wire_version = 1` on every request; expect a loud
   `wire_version_unsupported` error + close on mismatch.
2. Enforce the 64 MiB cap on the **length prefix**, before allocating.
3. One in-flight request per connection; read until a terminal frame
   (`done`/`error` for generation, `embeddings`/`error` for embed,
   `rerank`/`error` for rerank).
4. Ignore unknown JSON fields and unknown `type`/`kind`/`code` values
   (forward-compat) — do not hard-error on them.
5. Send attachment BLOBs in `attachments[]` order, each preceded by its
   descriptor; send raw decoded bytes, never base64.
6. Retry/fallback is yours — the daemon never retries, degrades, or
   fails over (ADR 0007). A dropped connection cancels the job.
7. `backend` is diagnostic; never branch on it.
