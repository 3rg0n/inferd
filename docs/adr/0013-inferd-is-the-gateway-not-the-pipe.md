# 0013. inferd is the gateway, not the pipe

- Status: accepted
- Date: 2026-05-20

## Context

Through alpha and the early v0.1.x cycle, inferd's positioning has
been described two different ways in different places, and the
inconsistency surfaces in design discussions. The two framings:

1. **inferd is a pipe.** The daemon ships NDJSON-over-IPC; the
   middleware (consumer) does *all* model-specific work — chat
   templating, image preprocessing, tokenization decisions, tool-
   call orchestration. The daemon just passes bytes from the wire
   to llama.cpp's input and back.

2. **inferd is a gateway.** The daemon owns model-specific
   shaping. Middleware sends semantic intent (`messages[]`,
   `attachments[]`, `tools[]`); the daemon translates that into
   what the engine actually needs (chat-formatted prompts, raw
   image bytes routed through llama.cpp's mtmd helpers, tool-call
   lifecycle managed). Middleware doesn't know that Gemma's chat
   format wraps turns in `<|turn>...<turn|>` tokens, doesn't know
   that the engine's mtmd API takes raw JPEG bytes via a side
   channel, doesn't know how the model emits `<|tool_call>...
   <tool_call|>` sequences.

These framings produce different downstream decisions. Under #1,
multimodal "fits in `String` content" — middleware base64-encodes
images into the prompt text. Under #2, multimodal needs typed
content blocks + attachment side-channels because that's what the
engine actually wants.

The pipe framing was an early simplification that has, on closer
inspection, been wrong since at least the ADR 0008 frozen-v1
work. Two reasons:

- **llama.cpp's multimodal interface (mtmd) requires raw image
  bytes alongside the prompt**, not base64 in the prompt text.
  The model's `<|image|>` placeholder tokens get *replaced* with
  encoded soft embeddings during inference, not parsed as
  literal characters. A consumer that puts JPEG bytes into the
  text content gets garbage tokens, not vision.
- **Every other LLM gateway works the same way.** Anthropic's
  `/v1/messages`, OpenAI's `/v1/chat/completions`, Bedrock's
  Converse API, Ollama's `/api/chat` — all take semantic
  `messages[]` with typed content blocks (text + image + tool +
  ...) and do the encoding server-side. Asking middleware to
  reimplement that translation per-engine is the kind of
  duplication inferd was created to prevent (rephrased: the same
  warm-model-lifecycle duplication that bothered us at the
  process layer also bothers us at the prompt-shaping layer).

## Decision

**inferd is an LLM gateway.** The daemon is responsible for the
mapping between semantic intent (what the consumer sends) and
engine input (what llama.cpp / a cloud backend / a future
multimodal model actually consumes).

Concretely, the daemon owns:

- **Chat templating.** Today's Gemma 4 hand-rolled wrap is the
  daemon's job. v0.2 generalises this through the `Backend`
  trait so each adapter applies its own engine's chat format.
- **Attachment routing** (v0.2). When a request includes binary
  blobs (image / audio / video), the daemon hands them to the
  engine via the engine's native side-channel — for llama.cpp
  that's the mtmd helpers; for cloud backends it's whatever the
  upstream API takes (typically base64-in-the-content-block).
- **Tool-call orchestration** (v0.2). The model emits
  `<|tool_call>...<tool_call|>` (Gemma) or its equivalent; the
  daemon parses that out of the raw token stream and emits a
  typed `Response::ToolCall` frame on the wire. The consumer
  executes the tool and sends a `tool_result` content block back.
  The daemon doesn't execute tools — it shapes the lifecycle.
- **Tokenization decisions** that are downstream of the chat
  template (e.g. truncating to `n_ctx`, deciding when to stop
  reading prompt because the engine's context is full).
- **Backend routing**, admission, lifecycle, security perimeter —
  unchanged from v0.1.

The consumer (thlibo, the inferd CLI, future IDE plugins, agent
runtimes, web apps) owns:

- **User experience** — input capture, rendering, session memory,
  what to keep across turns.
- **Acquiring the bytes** — image file picker, microphone
  capture, document parser, screenshot tool.
- **Format validation upstream of the wire** (size limits, MIME
  sniffing, polite resize-before-upload).
- **Tool execution** — the actual function bodies the model is
  invoking via tool-call frames. The daemon orchestrates the
  *lifecycle*; the consumer runs the *code*.
- **Authentication of the user** to the consumer (a concern
  separate from inferd's per-caller identity at the IPC layer).
- **Encoding to base64** when the wire format calls for it
  (v0.2 attachments).

This split mirrors Claude Code ↔ Anthropic API ↔ Anthropic's
models exactly:

```
[ middleware ]                  [ inferd daemon ]            [ engine + model ]
  thin client                     smart gateway                math
  - knows the user                - knows the model            - tokens in
  - knows the task                - knows the backend          - tokens out
  - sends semantic intent         - shapes intent → engine
                                  - routes attachments
                                  - orchestrates tool calls
```

The reason the work landed there in Anthropic's stack is the
same reason it should land there in inferd's: vision encoders
are huge model-specific weights, prompt formats are model-
specific contracts, and asking every consumer to know which
model is on the other side breaks the gateway abstraction.

## Consequences

**Why this is the right shape:**

- **Aligns with consumer expectations.** Middleware authors who
  have written against Anthropic / OpenAI / Bedrock can write
  against inferd with the same mental model. The wire shape is
  Anthropic-API-shaped (typed content blocks + attachments);
  the transport is just IPC instead of HTTPS.
- **Aligns with what llama.cpp actually wants.** mtmd is the
  intended multimodal entry point in llama.cpp upstream. Using
  it requires the daemon to take raw bytes — which is the
  gateway shape, not the pipe shape.
- **Keeps the consumer surface small.** The wire protocol grows
  typed content blocks in v2 but stays semantic. Middleware
  doesn't need to know Gemma's `<|tool_call>` format or the
  layout of a multimodal projector's input; it just sends
  semantic blocks.
- **Doesn't violate ADR 0006.** Lean-core was about HTTP servers,
  web UIs, and OpenAI-compat surfaces — *consumer-facing*
  things. Those still belong in ecosystem-extension processes.
  Model-specific *engine* shaping (chat templates, mtmd routing)
  is squarely a daemon concern; that's where engine knowledge
  lives. ADR 0006 stays load-bearing for the layer above; this
  ADR clarifies the layer below.

**What this costs:**

- The daemon takes on real protocol-translation work in v0.2.
  Backend adapters become non-trivial — each one has to know
  its engine's chat format, attachment model, and tool-call
  lifecycle. That's a much larger surface than "pass the wire
  bytes through to a Generate call."
- Wire protocol grows in v2 (typed content blocks, attachments,
  tools — see ADR 0015). Middleware that wants v0.2 features
  has to migrate from v1 string-content to v2 typed-content. v1
  stays frozen on its own socket per ADR 0008, so no breakage —
  but it does mean v2 middleware writes more JSON than v1.
- Consumers that want to do model-specific shaping themselves
  (e.g. for a research workflow that depends on a specific
  prompt format) are pushed to a different layer — they would
  bypass inferd and talk to llama.cpp directly. That's
  acceptable: inferd is the gateway for the common case, not a
  universal escape hatch.

**What this explicitly does not change:**

- **v1 wire protocol stays frozen.** No retroactive breakage.
  Today's text-only `Message { role, content: String }` keeps
  working on the v1 socket. ADR 0008 unchanged.
- **ADR 0006 (lean-core) stays.** HTTP / web UI / OpenAI-compat
  still don't go in the daemon. This ADR clarifies what *does*
  go in: model-specific shaping. Distinct from consumer-facing
  surfaces.
- **ADR 0007 (no in-daemon retry, no mid-stream failover) stays.**
  The router still picks a backend per operator policy and the
  daemon never invents recovery. Tool-call orchestration is
  *lifecycle*, not retry — the daemon emits a tool-call frame
  and waits for the consumer's tool-result; that's the contract,
  not a retry.
- **ADR 0011 (shared CAS model store) stays.** The store
  contains blobs and manifests; the daemon mmaps them. Vision
  projector layers (e.g. an mmproj GGUF file) become additional
  blobs in the store, with their own SHA in the manifest.
- **ADR 0012 (one warm model per inferd process) stays.** A
  multimodal Gemma 4 is still one model; the projector layers
  are part of it. The "one warm model" rule means one model
  family with its full set of weights (text + projector if
  multimodal), not "one text model and one image encoder."

## Alternatives considered

- **Stay pipe-shaped, push everything to middleware.** Rejected
  on the engine-coupling grounds above (mtmd needs raw bytes,
  not base64 in text). Also breaks consumer expectations
  (every other LLM gateway is gateway-shaped).
- **Hybrid: gateway for some operations, pipe for others.**
  Rejected as too clever. The pipe-vs-gateway distinction is
  load-bearing on the architecture; making it conditional means
  the daemon has two model-knowledge stories. Pick one.
- **Defer this decision until v0.2 starts.** Rejected: every
  v0.2 design discussion to date has been muddled by the
  pipe-vs-gateway ambiguity. Locking it now lets the v0.2
  protocol design (ADR 0015) and the second-backend work
  proceed on a clear footing.

## References

- ADR 0006 — lean-core posture (this ADR clarifies which layer
  it applies to).
- ADR 0007 — backend routing semantics (unchanged by this).
- ADR 0008 — protocol v1 frozen (unchanged; v2 lives separately).
- ADR 0011 — shared CAS model store (multimodal projector
  layers fit naturally as additional blobs).
- ADR 0012 — one warm model per inferd process (clarifies the
  "model family" framing).
- ADR 0014 (companion) — the inferd CLI is a reference
  middleware, not a privileged surface. Direct corollary of
  this one: if the daemon is a gateway and the CLI is just a
  consumer, the CLI uses the same library surface every other
  consumer does.
- ADR 0015 (companion) — v2 wire protocol shape. Concretises
  the typed content blocks + attachments + tools that this
  ADR commits to.
