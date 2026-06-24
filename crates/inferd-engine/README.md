# inferd-engine

Backend adapter crate. Defines the `Backend` trait that abstracts
over different inference sources and ships the adapters that implement
it: `llamacpp` (FFI to a vendored `llama.cpp`, the default — text +
multimodal via mtmd + embeddings), `mock` (tests), and the
feature-gated cloud adapters `openai-compat` and `bedrock-invoke`
(outbound HTTPS only, ADR 0006).

**Status: shipping (v0.5).** With the `dl-backends` feature (ADR 0019)
each ggml backend (CPU / Metal / CUDA / Vulkan / ROCm) builds as a
loadable module and the strongest available is selected at runtime.
The trait carries `generate_v2` (typed content blocks / attachments /
tools — the single generation surface; text-only is one `text` block)
and `embed`, each gated by a capability the daemon advertises via
`capabilities()`. The v1 text-only `generate` method was removed in
v0.4 when v1 folded into v2 (ADR 0021). As of v0.5, `generate_v2`
honours `response_format` (a JSON Schema) by compiling it to a GBNF
grammar so output is constrained to match the schema (ADR 0013).

See `../../docs/adr/` (0005 FFI, 0013 gateway shaping, 0015 v2, 0017
embeddings, 0019 runtime accelerator) for the design each adapter
satisfies.
