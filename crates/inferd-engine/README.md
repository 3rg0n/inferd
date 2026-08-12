# inferd-engine

Backend adapter crate. Defines the `Backend` trait that abstracts
over different inference sources and ships the adapters that implement
it: `llamacpp` (FFI to a vendored `llama.cpp`, the default — text +
multimodal via mtmd + embeddings + cross-encoder rerank), `mock`
(tests), and the feature-gated cloud adapters `openai-compat` and
`bedrock-invoke` (outbound HTTPS only, ADR 0006).

**Status: shipping (v0.8).** With the `dl-backends` feature (ADR 0019)
each ggml backend (CPU / Metal / CUDA / Vulkan / ROCm) builds as a
loadable module and the strongest available is selected at runtime.
The trait carries `generate_v2` (typed content blocks / attachments /
tools — the single generation surface; text-only is one `text` block),
`embed`, and `rerank`, each gated by a capability the daemon advertises
via `capabilities()`. `rerank` (ADR 0027) has a default implementation
returning `RerankError::Unsupported`, so an adapter opts in rather than
being broken by the addition. The v1 text-only `generate` method was
removed in v0.4 when v1 folded into v2 (ADR 0021). As of v0.5,
`generate_v2` honours `response_format` (a JSON Schema) by compiling it
to a GBNF grammar so output is constrained to match the schema
(ADR 0013). It enforces `tool_choice` (ADR 0029) the same way, through
`ChatRenderer::tool_call_grammar` — whose default implementation
**refuses every mode**, so a renderer family opts in deliberately instead
of inheriting a silently-unenforced `required`. The two are mutually
exclusive: one grammar sampler exists, so a request carrying both is
rejected rather than having one constraint dropped.

## Rerank needs its own context, and fails at load time

`pooling_type` is fixed when a `llama_context` is created, and rerank
requires `LLAMA_POOLING_TYPE_RANK` to attach the classification head —
so it cannot share the embed context, and `rerank` never implies `embed`.
The adapter builds a second context behind `LlamaCppConfig::rerank`
(default **off**: a rerank context plus its KV cache is real memory a
deployment doing no retrieval must not pay for).

Preconditions — BOS present, plus at least one of EOS / SEP / a model
`rerank` chat template — are checked at **load**, not per request. That
placement is the whole point: the classification head returns a float
whatever model is loaded, so a model without one yields *meaningless
scores* rather than an error. Failing the load means the daemon never
binds the rerank socket (invariant #5), which is the only way a caller
can tell. Scores are returned raw — one forward pass per document, KV
cache cleared between pairs, sorted descending by `total_cmp`, then
truncated to `top_n`.

## Capabilities are advertised, not assumed

`BackendCapabilities` is how the daemon learns what the loaded model
can do — `vision`, `audio`, `embed`, `rerank`, `tools`, `thinking` — and it is
also how a *requirement* reaches the consumer. `audio_sample_rate`
carries the one rate the loaded projector accepts (16000 for the
reference Gemma 4 E4B mmproj); the `llamacpp` adapter **rejects** an
`Attachment::Audio` at any other rate and never resamples, because
libmtmd's audio entry point takes no rate argument — feeding it the
wrong rate time-scales the clip and produces a fluent wrong answer
rather than a detectable error (ADR 0016). Adapters that add a modality
must advertise it *and* any such requirement; a capability left
unadvertised is unreachable, and a requirement left unadvertised is a
silent-wrong-output bug.

See `../../docs/adr/` (0005 FFI, 0013 gateway shaping, 0015 v2, 0016
consumer decodes media, 0017 embeddings, 0019 runtime accelerator, 0027
rerank) for the design each adapter satisfies.
