# inferd-engine

Backend adapter crate. Defines the `Backend` trait that abstracts
over different inference sources and ships the adapters that implement
it: `llamacpp` (FFI to a vendored `llama.cpp`, the default — text +
multimodal via mtmd + embeddings), `mock` (tests), and the
feature-gated cloud adapters `openai-compat` and `bedrock-invoke`
(outbound HTTPS only, ADR 0006).

**Status: shipping (v0.3).** With the `dl-backends` feature (ADR 0019)
each ggml backend (CPU / Metal / CUDA / Vulkan / ROCm) builds as a
loadable module and the strongest available is selected at runtime.
The trait carries `generate` (v1 text), `generate_v2` (typed content
blocks / attachments / tools), and `embed`, each gated by a
capability the daemon advertises.

See `../../docs/adr/` (0005 FFI, 0013 gateway shaping, 0015 v2, 0017
embeddings, 0019 runtime accelerator) for the design each adapter
satisfies.
