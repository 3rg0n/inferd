# 0017. Embeddings on a third socket — NDJSON, not HTTP

- Status: accepted
- Date: 2026-05-21

## Context

v0.2.0 lights up generation surfaces on two sockets:

- `infer.sock` / `\\.\pipe\inferd-infer` — v1 (text-only,
  per ADR 0008).
- `infer.v2.sock` / `\\.\pipe\inferd-infer-v2` — v2 (typed
  content blocks, attachments, tools, per ADR 0015).

Issue #10 needs a third surface: embeddings. The motivating
consumer is thlibo's cordon-filter k-NN anomaly scorer, which
needs vectors for short text inputs. The default model in flight
is `embeddinggemma-300m` (Google EmbeddingGemma 300M, GGUF via
unsloth), which is an embedding-only model — it does not
generate tokens.

Three live questions:

1. **Transport.** Embedding consumers in the wild use HTTP
   (`/v1/embeddings`, /v1/embeddings on Bedrock Titan, etc.).
   Should inferd expose embeddings over HTTP for parity?
2. **Endpoint shape.** Add `embed: true` to the existing v1 / v2
   request frames? Reuse the existing socket? Bind a separate
   one?
3. **Multi-model.** EmbeddingGemma is a different model from
   any generation model. Does serving embeddings require a
   second inferd process, per ADR 0012?

## Decision

### Transport: NDJSON-over-IPC, not HTTP

Embeddings ship on **a separate IPC socket** in the same shape
as v1 and v2: NDJSON-over-UDS / named-pipe / loopback-TCP.
ADR 0006 forbids HTTP in the daemon; ADR 0008 forbids in-band
version negotiation. Both apply here unchanged. `/v1/embeddings`-
shaped HTTP is an ecosystem-extension job (an `inferd-http`
adapter process, mirroring the OpenAI-compat pattern), not a
daemon job.

### Endpoint: a third dedicated socket

| Platform | Embed socket |
|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.embed.sock` |
| macOS | `${TMPDIR}/inferd/infer.embed.sock` |
| Windows | `\\.\pipe\inferd-infer-embed` |

The socket is bound **only when** the backend in this daemon
exposes embedding capability. A daemon serving an
embedding-only model (e.g. EmbeddingGemma) binds the embed
socket and does **not** bind v1 / v2 generation sockets — the
backend's `supports()` reports text=false, embed=true, and the
daemon's listener wiring follows the capability advertisement.
Conversely, a daemon serving a generation-only model (e.g.
gemma-4-e4b) binds v1 / v2 and not the embed socket.

The admin socket is shared across all three surfaces — admin
is about daemon lifecycle, not request shaping.

TCP exposure follows the same opt-in shape as v1 / v2: a
`listen.tcp_embed` config field plus matching CLI flag
(`--listen-tcp-embed`), default off, API-key-by-env required
when set.

### Wire format

#### Embed request

```json
{
  "id": "req-001",
  "input": ["passage one", "passage two"],
  "dimensions": 768,
  "task": "retrieval_document"
}
```

Required: `id`, `input` (non-empty array of non-empty strings,
each ≤ model context).

Optional:
- `dimensions` — Matryoshka truncation length. EmbeddingGemma
  supports `768 | 512 | 256 | 128`. Omitted means "model
  default" (768 for EmbeddingGemma). Daemon emits
  `invalid_request` if the value is not in the model's
  supported set.
- `task` — task-prefix hint for models trained with task-aware
  prefixes (EmbeddingGemma supports `retrieval_query`,
  `retrieval_document`, `classification`, `similarity`,
  `clustering`, etc.). The daemon applies the engine-specific
  prefix on behalf of the consumer (per ADR 0013 — daemon owns
  model-specific shaping). Backends that don't recognise the
  task ignore the field.

Unknown fields are ignored on parse — additive forward
compatibility, same posture as v1 / v2.

#### Embed response

Single frame, terminal:

```json
{
  "type": "embeddings",
  "id": "req-001",
  "embeddings": [
    [0.0123, -0.0456, "..."],
    [0.0234, -0.0567, "..."]
  ],
  "dimensions": 768,
  "model": "embeddinggemma-300m",
  "usage": {"input_tokens": 12},
  "backend": "llamacpp"
}
```

`embeddings` is an array-of-arrays of `f32` values, one inner
array per input string, in the same order as the request's
`input`. `dimensions` is the actual length of each inner array
after any MRL truncation.

`backend` is the `Backend::name()` that served the request, per
ADR 0007's observability requirement (same field as v1 / v2
`done` frames).

The connection stays open for the next request — same
long-lived shape as v1 / v2 inference connections.

#### Embed error

Same envelope as v1 / v2 errors:

```json
{
  "type": "error",
  "id": "req-001",
  "code": "invalid_request",
  "message": "dimensions must be one of [128, 256, 512, 768]"
}
```

`code` values: `queue_full | backend_unavailable |
invalid_request | internal` (same as v1) plus
`embed_unsupported` for the case where a daemon configured
with a generation-only backend receives an embed request
(belt-and-braces — the embed socket should not have been bound
in the first place if the backend can't embed).

### Frame cap

Same 64 MiB per-frame cap as v1 / v2. A 768-dim `f32` vector
serialised as JSON is ~10 KiB; one 64 MiB response frame
admits ~6,500 vectors per call — sufficient for batched k-NN
ingest.

### Multi-model is still many processes

ADR 0012 (one warm model per inferd process) stands. An
embedding model is a separate model from any generation model.
Operators who want both embeddings AND generation run two
inferd processes:

- inferd #1: `~/.inferd/config-gen.json` →
  `\\.\pipe\inferd-infer` + `inferd-infer-v2` (no embed).
- inferd #2: `~/.inferd/config-embed.json` →
  `\\.\pipe\inferd-infer-embed` (no v1 / v2 generation).

This is the same pattern as running two redis instances on
different sockets, or two postgres clusters on different
ports. Each daemon keeps its lean-core posture (ADR 0006).

A future model that natively supports both embedding and
generation in one weight (some BERT-style hybrids do) would
bind all three sockets — the trait already accommodates this
via independent capability flags. v0.2.0 does not ship such a
model; the case is named here only to confirm the design
permits it.

### v0.2.0 scope: llamacpp only

The v0.2.0 implementation:

- `inferd-engine`'s `Backend` trait grows an `embed` method
  alongside `generate` / `generate_v2`. Default impl returns
  `unimplemented` (`embed_unsupported`). Backends opt in.
- `inferd-engine`'s `llamacpp` adapter implements `embed` via
  `llama_embed_seq` on the existing FFI bridge.
- `inferd-daemon` binds the embed socket when the active
  backend reports `supports().embed == true`.
- `inferd-client` adds an `EmbedClient` surface mirroring
  `Client` / `ClientV2`.
- `inferd-proto` adds `EmbedRequest` / `EmbedResponse` types.

Deferred to v0.2.1+ (explicit non-goals of this ADR):

- `openai-compat` backend's `/v1/embeddings` adapter.
- `bedrock-invoke` backend's Titan Embed support.
- Cross-backend routing for embeddings (an embed-capable
  primary + an embed-capable fallback). Same shape as
  generation routing per ADR 0007 once two adapters exist.

### Embedding capabilities frame (admin)

The capabilities frame on the admin socket (#77) gains an
`embed: bool` field, sibling to `vision`, `audio`, `tools`,
`thinking`. It reports whether the active backend exposes
embedding. The daemon publishes one capabilities frame to
admin at startup; consumers and `inferd doctor` read it to
decide whether the embed socket is worth dialling.

## Consequences

**Why this is the right shape:**

- **No new protocol concepts.** Same NDJSON framing, same 64
  MiB cap, same long-lived connection, same error envelope as
  v1 / v2. Consumers writing for inferd already speak this
  shape; a third socket is "do the same thing on a different
  path."
- **ADR 0012 stays untouched.** Embeddings don't introduce
  multi-model warm pools. Operators who need both gen and
  embed run two daemons, the same way they'd run two redis
  instances.
- **ADR 0006 stays untouched.** No HTTP. The
  `/v1/embeddings`-shaped consumer surface is an ecosystem
  extension, not a daemon feature.
- **Capability-driven socket binding** keeps the wire surface
  honest — a daemon advertises only the surfaces it can
  actually serve. A consumer dialling `infer.embed.sock` on a
  generation-only daemon gets a connection refused, not a
  silent timeout or a runtime error.
- **MRL truncation lives at the consumer's request,** not in
  middleware shims. EmbeddingGemma's MRL training is a wire
  parameter, applied by the daemon (per ADR 0013 — daemon
  owns model-specific shaping).

**What this costs:**

- Operators who want both gen and embed manage two daemon
  processes, two configs, two pipes / sockets. Same cost as
  the multi-model story already paid in ADR 0012.
- The `Backend` trait gains a method. Existing `mock` and
  `llamacpp` adapters need an `embed` impl (mock returns
  fixtures; llamacpp wires `llama_embed_seq`). The
  `bedrock-invoke` and `openai-compat` adapters ship without
  `embed` for v0.2.0 — they return `embed_unsupported` until
  the v0.2.1+ work lands.
- One more wire surface to keep frozen. Once v0.2.0 ships,
  the embed envelope is as immutable as v1 is — additive
  forward changes only, breaking changes go to a successor
  socket. This is the same posture as ADR 0008 / ADR 0015.

**What this explicitly does not change:**

- ADR 0006 — daemon stays HTTP-free.
- ADR 0007 — no in-daemon retry. An embed call that hits a
  failed backend returns `backend_unavailable`; the consumer
  retries.
- ADR 0008 — v1 generation socket frozen, separate.
- ADR 0011 — embedding model blobs live in the same CAS store
  as generation model blobs. The manifest's `name` field
  distinguishes them; nothing else needs to change in the
  store layout.
- ADR 0012 — still one warm model per process. The "three
  sockets" framing is about wire surfaces, not model count.
- ADR 0015 — v2 generation contract untouched.

## Alternatives considered

- **Add `task: "embed"` to v1 or v2 requests; reuse one
  socket.** Rejected. v1 is frozen (ADR 0008); v2 is locked
  (ADR 0015). Adding an embed branch to either request type
  conflates two operationally-different shapes (token-stream
  vs single-frame vector) into one parser, and re-opens the
  versioning question. A separate socket is cleaner and
  matches the "v2 went on a separate socket" precedent.
- **Expose `/v1/embeddings` HTTP from the daemon.** Rejected
  on ADR 0006 grounds. The OpenAI-compat HTTP shape is
  exactly the "ecosystem extension lives in a separate
  process" pattern ADR 0006 already accepts; no exception
  warranted for embeddings.
- **One inferd serves both gen and embed (relax ADR 0012 for
  the embed case).** Rejected. ADR 0012's "one warm model"
  rule isn't about *what* the model does; it's about *how
  many* model artefacts the daemon holds resident. A daemon
  loading both `gemma-4-e4b` (5 GB) and `embeddinggemma-300m`
  (300 MB) is two warm models, full stop. The multi-process
  pattern is the documented answer.
- **Defer embeddings to v0.3.** Rejected. Issue #10 has a
  consumer (thlibo) blocked on this; the wire shape is
  small; the engine work (`llama_embed_seq`) is well-
  understood. Cost of carrying it in v0.2.0 is low; cost of
  blocking thlibo's k-NN scorer for another release is
  higher.

## References

- ADR 0006 — daemon stays HTTP-free; `/v1/embeddings` is an
  ecosystem extension.
- ADR 0007 — backend routing semantics; embeddings reuse them
  unchanged.
- ADR 0008 — v1 generation frozen; separate-socket-per-version
  rule generalised to separate-socket-per-surface here.
- ADR 0011 — shared CAS model store; embedding blobs fit
  unchanged.
- ADR 0012 — one warm model per process; multi-model needs
  multi-process, including the embed-vs-gen split.
- ADR 0013 — daemon owns model-specific shaping (task
  prefixes, MRL truncation applied here, not in middleware).
- ADR 0015 — v2 generation contract; this ADR is the
  embedding sibling.
- Issue #10 — thlibo cordon-filter k-NN anomaly scoring (the
  motivating consumer).
- EmbeddingGemma 300M model card —
  `https://huggingface.co/unsloth/embeddinggemma-300m-GGUF`.
