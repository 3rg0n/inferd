# How local LLM serving actually works

A working "AI" on a laptop is not one thing. It is a stack of about
fifteen components that each do one job, layered on top of each other,
glued together by a daemon that holds them in memory. People say "the
model" and gesture at the whole stack, but the model is only one
component. Everything else has to be present and correct or nothing
runs.

This document walks the stack bottom-up — from the bytes on disk to
the tokens streaming out a socket — and explains what each component
does, why it exists, and what fails if it is missing or wrong. At the
end it maps the components onto **llamafile** (a popular real-world
implementation) so the abstract picture lands on something concrete.

The goal is for a reader who has never opened a model serving
codebase to come away with a clear mental model of *what the parts
are* and *why each one is non-optional*.

## The 15 components

### 1. The model weights (the GGUF file)

**What it is**: A binary file, typically 1–50 GB, containing the
trained neural network's parameters — billions of floating-point
numbers organized into matrices.

**Why you need it**: This is *the* model. Without it there is
nothing to run. Gemma 4 E4B's weights are what was learned during
training; the file is a snapshot of that knowledge.

**Format detail**: GGUF is a specific file format that has become
the dominant container for local CPU/laptop inference. It packages:

- The weight tensors (often quantized — see §3)
- Model architecture metadata (layer count, attention heads, vocab
  size, etc.)
- The tokenizer (see §2)
- The chat template (see §10)

You don't *need* GGUF specifically — HuggingFace uses safetensors,
PyTorch uses `.pth` — but GGUF is the dominant format for local
inference because it bundles everything into one file and supports
aggressive quantization.

### 2. The tokenizer

**What it is**: A lookup table that converts text ↔ integers.
`"Hello world"` → `[1234, 567]`. Every model has its own vocabulary
(typically 32k–256k entries) baked in at training time.

**Why you need it**: The neural network operates on integers, not
text. You must tokenize input before inference and de-tokenize
output. Wrong tokenizer = garbage output (the integers map to
different words than the model was trained on).

**Implementation detail**: BPE (Byte-Pair Encoding) or SentencePiece
are the algorithms. Fast tokenizers are usually written in Rust or
C++ because they run on every request.

### 3. Quantization

**What it is**: Compressing each weight from 16 or 32 bits down to
4, 5, 6, or 8 bits. A 4 GB model at fp16 becomes ~1 GB at 4-bit
quantization.

**Why you need it**: Without quantization, a 4B parameter model
would need ~8 GB of RAM and would be slow on CPU. Quantized to
Q4_K_M (a specific scheme), it fits in 3 GB and runs 2–4× faster on
CPU. This is what makes "run an LLM on a laptop" feasible at all.

**Tradeoff**: Some quality loss. Aggressive quantization (Q2, Q3)
noticeably degrades output. Q4_K_M and Q5_K_M are the sweet spot
most people ship.

### 4. The inference engine (the forward pass)

**What it is**: The code that actually runs the model. Takes input
tokens + KV cache + weights, produces a probability distribution
over the next token. This is where the matrix math happens:

- Embeddings lookup
- N transformer layers, each doing attention + feed-forward
- Final layer normalization
- Output projection to vocab logits

**Why you need it**: This *is* the model running. Without it, the
GGUF file is just bytes.

**Where the work is**: Implementing attention correctly, including
positional encoding (RoPE for Gemma), grouped-query attention,
sliding-window attention for long context, etc. Each model
architecture has quirks that the engine has to account for.

### 5. The compute kernels (matmul, softmax, RoPE, etc.)

**What it is**: The hot inner loops. Matrix multiplication is 90%+
of inference time. To make it fast you need code specialized for
each CPU instruction set (AVX2, AVX-512, NEON, AMX) and each GPU
architecture (CUDA, Metal, Vulkan, ROCm).

**Why you need it**: A naive matmul is 100× slower than a tuned
one. The difference between "5 tokens/sec" and "50 tokens/sec" on
the same hardware is kernel quality.

**Implementation detail**: Modern stacks ship one matmul file per
ISA × operation — for example a separate file for AVX2 fp16, a
separate file for AVX-512 int8, a separate file for ARM NEON int4.
Quantized matmul kernels operate directly on the compressed weights
without dequantizing them first; that is where most of the speed
on consumer CPUs comes from.

### 6. The KV cache

**What it is**: As the model generates token by token, it reuses
computation from previous tokens. The "key" and "value" tensors
from each attention layer are saved per-token so subsequent tokens
don't recompute them.

**Why you need it**: Without a KV cache, generating token N requires
re-running all previous N-1 tokens. With it, only token N is
computed. This is the difference between O(n²) generation and O(n)
generation. Mandatory for any usable system.

**Memory cost**: KV cache for a 4B model at 8k context can be 1–2
GB. This is why long contexts get expensive.

### 7. The sampler

**What it is**: The model outputs a probability over the next token
(vocab-sized, ~256k for some models). The sampler picks one.
Strategies:

- **Greedy** — always the highest probability. Deterministic but
  boring.
- **Temperature** — flatten or sharpen the distribution before
  sampling.
- **Top-k** — only consider the top k candidates.
- **Top-p (nucleus)** — only consider the smallest set summing to
  probability p.
- **Min-p** — newer, often better than top-p.

**Why you need it**: This controls how creative vs deterministic
the output is. The `temperature`, `top_p`, `top_k` fields you see
in any chat API are exactly this.

### 8. Grammar-constrained sampling (GBNF)

**What it is**: After the sampler computes the probability
distribution, *mask out* every token that isn't allowed by a formal
grammar. Renormalize. Sample from what's left.

**GBNF** stands for "GGML Backus-Naur Form" — a variant of BNF, the
standard notation for context-free grammars. Example:

```gbnf
root   ::= "{" ws "\"name\":" ws string "," ws "\"age\":" ws number ws "}"
string ::= "\"" [^"]* "\""
number ::= [0-9]+
ws     ::= [ \t\n]*
```

This grammar says "the only valid output is JSON with a `name`
(string) and `age` (number)." With this grammar attached, **the
model cannot produce invalid JSON**, period. Not "tries hard not
to" — literally can't, because at every step, tokens that would
break the grammar have their probability set to zero before
sampling.

**Why you need it**: When a downstream program is going to *parse*
the model's output — extract a JSON field, run a tool call, fill a
form — "hopefully the prompt was clear enough" is not good enough.
GBNF makes the structure mathematically guaranteed. This is the
feature that gives "constrained decoding," "JSON mode," and "tool
calling" their reliability. OpenAI's "JSON mode" is the same idea
hidden behind a simpler interface.

**How it's implemented**: A grammar parser plus, at every
generation step, walking the grammar's state machine and checking
which next tokens are reachable. Non-trivial but well-understood.
The cost is a per-step grammar check, cheap relative to a forward
pass.

**Caveat**: Not every inference engine ships GBNF. It is one of the
features that makes a local stack actually *useful* for programmatic
work, not just chat.

### 9. Stop conditions

**What it is**: Knowing when to stop generating. Conditions:

- Hit `max_tokens` limit (caller-specified)
- Generated the model's end-of-turn token (e.g.,
  `<end_of_turn>` for Gemma)
- Caller-specified stop strings ("\n\n", "User:", etc.)
- Grammar reaches a terminal state (with GBNF)
- Client disconnects (cancellation)

**Why you need it**: Without stop conditions the model generates
forever (until `max_tokens`). For chat the natural stop is the EOT
token; for structured output it's the grammar saying "done."

### 10. The chat template

**What it is**: A small templating language (Jinja2 in HuggingFace,
ad-hoc strings in GGUF) that formats a list of `{role, content}`
messages into the exact string the model was trained to expect.
Gemma's looks like:

```
<start_of_turn>user
Hello<end_of_turn>
<start_of_turn>model
```

**Why you need it**: Models are trained on specific role-formatting
tokens. If you concatenate messages without the template, the
model has no idea where the user ends and the assistant should
begin — output quality collapses. Wrong template = subtly worse
output that's hard to debug.

**In GGUF**: bundled in the file. A well-behaved engine applies it
automatically when you pass a messages array.

### 11. The model loader / lifecycle

**What it is**: Code that:

- Verifies the GGUF file (SHA-256 — and the verification should be
  *constant-time* to avoid timing leaks during checks against an
  expected hash)
- Memory-maps it into the process
- Allocates KV cache
- Initializes GPU buffers if applicable
- Reports "ready" when all of that is done
- Handles graceful shutdown / restart

**Why you need it**: Loading a model takes 5–30 seconds. You want
this to happen *once*, at daemon startup, not per-request. This is
the entire reason a serving daemon exists — keep one model warm.

### 12. Request scheduler / admission control

**What it is**: Manages multiple concurrent requests against a
single model. A common simple design: 1 active generation + N
queued, FIFO, non-blocking submit.

**Why you need it**: A single model instance can do *one* forward
pass at a time (or N with batching, but that's harder). If 10
clients hit it simultaneously and you don't queue, they all try to
share the same KV cache and corrupt each other.

**Advanced version**: Continuous batching — interleave tokens from
multiple requests through the model in a single forward pass.
Doubles or triples throughput. vLLM and TGI do this. llama.cpp has
a simpler version. This is the path to multi-client throughput
when single-active-generation gets saturated.

### 13. The transport / protocol

**What it is**: How clients talk to the daemon. Common choices:

- HTTP with OpenAI-compatible JSON endpoints (the default for most
  serving stacks)
- NDJSON (newline-delimited JSON) over a Unix domain socket,
  Windows named pipe, or loopback TCP
- gRPC for typed streaming

**Why you need it**: Without it, clients can't ask for inferences.
This layer defines request/response shapes, frame caps, image
budget validation, cancellation semantics.

**Note on loopback HTTP**: When a daemon serves over `127.0.0.1`,
traffic still traverses the kernel's TCP/IP stack — socket buffers,
TCP state, framing. It is real overhead and a real perimeter.
Unix domain sockets and named pipes avoid the network stack
entirely; for a local-only daemon they are usually the better
default.

### 14. Observability

**What it is**: Logs, metrics, traces. NDJSON activity logs are a
common shape — one record per request with timestamps, params,
outcomes, latencies — written to a rolling file with rotation
(typically keep 3 generations) and a redactor that scrubs anything
that looks like a credential before write.

**Why you need it**: When something breaks at 2am you need to know
which request, what params, what the model said, how long it took.
Without observability you're debugging blind.

### 15. Security perimeter

**What it is**: Identity (UID on Unix, SID on Windows), socket
modes (e.g. `0660`), single-instance lock files (with
pre-existing-symlink rejection), frame size caps (typically 64
MiB), redaction at write time, constant-time hash compares for
model verification.

**Why you need it**: A daemon running on a multi-user machine,
holding tokens that may include the user's source code, diffs, or
credentials, with a multi-MiB frame surface, is a target. Each of
these controls closes a specific class of attack — TOCTOU,
impersonation, log scraping for secrets, denial-of-service via
oversized frames.

## Putting it together — example: llamafile

llamafile is a popular real-world implementation. It distributes a
local LLM as a single cross-platform executable. Mapping the 15
components onto its repo:

| # | Component | Where it lives in llamafile |
|---|---|---|
| 1 | GGUF format | `llama.cpp/` (vendored) |
| 2 | Tokenizer | `llama.cpp/` (vendored) |
| 3 | Quantization (K-quants) | `llama.cpp/` (vendored) |
| 4 | Forward pass | `llama.cpp/ggml*` (vendored) |
| 5a | CPU matmul kernels | `llamafile/tinyblas_*`, `iqk_mul_mat_*` |
| 5b | GPU kernels | `llama.cpp/ggml-cuda`, `ggml-metal`, etc. |
| 6 | KV cache | `llama.cpp/` (vendored) |
| 7 | Sampler | `llama.cpp/` (vendored) |
| 8 | GBNF grammar | `llama.cpp/` (vendored) |
| 9 | Stop conditions | `llama.cpp/` (vendored) |
| 10 | Chat template | `llama.cpp/` (vendored) |
| 11 | Model loader | `llama.cpp/` + `llamafile/main.cpp` |
| 12 | Scheduler | `llama.cpp/` server mode |
| 13 | HTTP transport | `llama.cpp/` server |
| 14 | Observability | basic logging |
| 15 | Single-binary packaging | `llamafile/cosmocc-*`, build scripts |

A few useful observations from this map:

- **The big building block is `llama.cpp`.** Most components 1–13
  are inherited from it. Any project that wants the "C++ engine
  with a stable contract" usually vendors or links llama.cpp.
- **Original work in llamafile concentrates in two places**: highly
  tuned CPU matmul kernels (component 5a, the `tinyblas_*` family
  with one file per CPU ISA) and the single-binary packaging
  machinery (component 15, the Cosmopolitan / APE integration).
  Both are real engineering, and both are *optimization and
  packaging around* an existing engine, not a re-implementation
  of it.
- **Kernel work is per-ISA × per-operation.** That is why the
  `tinyblas_cpu_*_amd_avx2.cpp`, `*_amd_avx512f.cpp`,
  `*_amd_zen4.cpp`, `*_arm80.cpp`, `*_arm82.cpp` files exist —
  each combination of CPU instruction set and matmul shape gets
  its own file so the compiler can fully unroll and intrinsify.

## What "owning the stack" actually means

When someone says "I want to own the inference stack in language X,"
the honest question is *which components*. Three meaningful
levels, each a different commitment:

- **Own components 11–15** (lifecycle, scheduling, transport,
  observability, security). This is the daemon perimeter. A few
  thousand lines, a few weeks of work. Achievable in any modern
  systems language and what most "serving daemon" projects mean.
- **Own components 11–15 in your language, depend on a vendored
  engine for 1–10.** Either via FFI to llama.cpp or by depending
  on a same-language inference library (candle, mistral.rs in
  Rust; tinygrad, mlx in Python). Tight coupling, no subprocess,
  but you inherit the engine's feature set — including whether or
  not it ships GBNF.
- **Own components 1–15 from scratch.** A from-scratch transformer
  runtime with multi-architecture kernels, quantization formats,
  GBNF, GGUF compatibility, and CUDA/Metal/Vulkan/ROCm GPU paths.
  This is the llama.cpp scope: years of effort, hundreds of
  contributors, multi-thousand-file codebase. Not a weekend
  project. Not even a quarter project.

The first level is what every serving daemon does. The second
level is the practical "owned in our language" answer. The third
level is rarely the right call unless the engine itself is the
product.

## Why this matters for system design

The 15 components are not interchangeable. Each one constrains the
others:

- The **tokenizer (§2)** and the **chat template (§10)** must
  match the weights (§1). They ship in the GGUF file for a reason.
- The **KV cache (§6)** sizing is dictated by the architecture in
  the weights file — you can't pick it independently.
- The **sampler (§7)** and **grammar (§8)** sit on top of the
  forward pass (§4) but are independent of which engine produces
  the logits — meaning grammar-constrained sampling can in
  principle be added to any engine that exposes a logits hook.
- The **scheduler (§12)** and the **transport (§13)** are the only
  components a daemon author has full freedom to design — every
  component below them is dictated by the model.

Designing a serving daemon is mostly designing components 11–15
well, then choosing carefully which engine provides 1–10. The
engine choice determines which features (quantization formats,
GBNF support, multimodal capability, GPU backend coverage) are
available. Everything above that — lifecycle, scheduling,
transport, observability, security — is the part the daemon
actually owns.

If a single takeaway lands, let it be this: **"AI" on a local
machine is fifteen distinct concerns wearing a trench coat**, and
the engineering value lives in how cleanly you separate them.
