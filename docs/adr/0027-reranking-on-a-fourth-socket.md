# 0027. Reranking on a fourth socket — cross-encoder scores, not vectors

- Status: accepted
- Date: 2026-08-04

## Context

Embeddings (ADR 0017) ship the *recall* half of retrieval: a
bi-encoder maps query and document to vectors independently, and
cosine similarity ranks them. That independence is what makes an
embedding index possible — documents are encoded once, offline — and
it is also its ceiling. The query never sees the document, so the
score cannot depend on their interaction. Negation, qualifiers, and
narrow intent are exactly the signals that live in the interaction,
and they are the ones bi-encoders lose: "config that does *not*
require a GPU" embeds close to "config that requires a GPU".

A cross-encoder fixes that by scoring the pair **jointly** — query
and document go through one forward pass together, and the model
emits a single relevance score through a classification head. It
cannot be indexed (there is no per-document vector to precompute),
so it is not a retrieval mechanism. It is the *reordering* second
stage over a candidate set the embedding index already narrowed:

```
embed → top-50 candidates → rerank(query, 50 docs) → top-5 → LLM
```

Issue #171 asks for this surface. inferd already holds the pieces:
the embed socket proves the single-frame NDJSON shape, and
`llama.cpp` at the vendored `b9850` has a first-class rerank path
that our existing bindgen allowlist already exposes.

Four questions to settle.

1. **Is the engine path real?** A wire surface we cannot serve is
   worse than none.
2. **Does this break ADR 0012** (one warm model per process)? A
   reranker is a third model role after generate and embed.
3. **Whose job is the pair formatting** — consumer or daemon?
4. **Does it cost anything when unused?** Most consumers do no
   retrieval at all.

## Decision

### The engine path exists and is already reachable

Verified against `vendor/llama.cpp` at `b9850`, not assumed:

- `LLAMA_POOLING_TYPE_RANK = 4` "attach[es] the classification head
  to the graph" (`include/llama.h:177`).
- With that pooling type, `llama_get_embeddings_seq` "returns
  `float[n_cls_out]` with the rank(s) of the sequence"
  (`include/llama.h:1030`) instead of an embedding vector.
- `llama_model_n_cls_out(model)` gives that width
  (`include/llama.h:576`); `llama_model_cls_label(model, i)` names
  each output when the model provides labels.
- Every symbol needed is already generated: `build.rs` allowlists
  `llama_.*` / `LLAMA_.*` wholesale, so no bindgen change is part of
  this work.

This makes rerank a **pooling variant of the embed context**, not a
new engine subsystem. The adapter already parameterises
`embed_pooling` (defaulting to `MEAN`); a rerank context is the same
allocation with `pooling_type = RANK`.

`common/common.cpp:1373-1394` documents preconditions upstream treats
as fatal, and so do we — checked at load, not per request:

- the vocab **must** have a BOS token, else "reranking will not work";
- it must have at least one of EOS, SEP, or a `rerank` chat template;
- EOS missing but SEP present is a warning, not an error (SEP is used
  as the fallback separator).

A model failing those checks fails backend init with a message naming
the missing token, rather than binding a socket that returns
plausible-looking garbage scores.

### A fourth dedicated socket

| Platform | Rerank socket |
|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.rerank.sock` |
| macOS | `${TMPDIR}/inferd/infer.rerank.sock` |
| Windows | `\\.\pipe\inferd-infer-rerank` |

NDJSON, single-frame request, single-frame response, 64 MiB cap,
long-lived connection, same error envelope — every framing decision
is inherited from ADR 0017 unchanged. Bound **only when** the active
backend advertises `capabilities().rerank`, exactly as the embed
socket is. The admin socket stays shared.

This follows the "separate socket per surface" rule rather than
adding a mode flag to the embed request. Embed returns
`Vec<Vec<f32>>` keyed to `input[]`; rerank returns scored indices
keyed to `documents[]`. One parser serving both would branch on a
mode field through validation, dispatch, and response construction —
and the embed envelope is frozen, so the branch could only be added
as an optional field whose absence changes the response *type*. That
is precisely the shape ADR 0017 rejected when it declined to put
`embed` behind a flag on the v1/v2 request.

### Wire format

#### Rerank request

```json
{
  "id": "req-001",
  "query": "how do I disable the GPU?",
  "documents": ["set n_gpu_layers to 0", "install the CUDA toolkit"],
  "top_n": 5
}
```

Required: `id`, `query` (non-empty), `documents` (non-empty array of
non-empty strings).

Optional:
- `top_n` — return only the `n` highest-scoring results. Omitted
  returns all. `0` is `invalid_request` (an empty result is never
  what a caller wants; silently returning nothing would look like a
  backend failure).

Bounds, enforced at the proto layer:
- `MAX_RERANK_DOCUMENTS` = **256** documents per request.
- `MAX_RERANK_TOTAL_BYTES` = **8 MiB** across `query` + all
  `documents`.

These exist because rerank is the one surface whose cost is
`O(documents)` **forward passes** — not one pass over a batch, as
embed is. A 64 MiB frame of short documents is ~500k pairs, and each
pair is a full model evaluation holding the shared admission permit.
The frame cap alone bounds bytes but not work, and this is the same
class of amplification as THREAT_MODEL F-1 (attachment bounds), where
one cheap frame entitled the sender to unbounded expense. Rejecting
at parse is cheap; discovering it at document 400,000 is not.

#### Rerank response

Single frame, terminal:

```json
{
  "type": "rerank",
  "id": "req-001",
  "results": [
    {"index": 0, "score": 0.98},
    {"index": 1, "score": 0.02}
  ],
  "model": "bge-reranker-v2-m3",
  "usage": {"input_tokens": 42},
  "backend": "llamacpp"
}
```

`results` is sorted by `score` **descending**, and `index` refers
back into the request's `documents` array. Sorting is the daemon's
job, not the consumer's: the score scale is model-specific (some
rerankers emit logits, others sigmoid probabilities), so
descending-by-score is the only ordering every model agrees on, and
making the consumer sort invites each one to re-derive it.

`index` is retained rather than echoing document text — the caller
already holds the documents, and echoing them could multiply an 8 MiB
request into an 8 MiB response for no information gain.

Scores are **not** normalised or comparable across models, and are
documented as such. A model emitting `n_cls_out > 1` uses output `0`
as the relevance score; the remaining outputs are not exposed (no
shipped reranker needs them, and inventing a multi-label wire shape
for a hypothetical model would freeze a guess).

#### Rerank error

```json
{
  "type": "error",
  "id": "req-001",
  "code": "invalid_request",
  "message": "documents must not be empty"
}
```

`code` values mirror embed's taxonomy — `queue_full |
backend_unavailable | invalid_request | frame_too_large | internal` —
plus `rerank_unsupported`, the fail-safe for a rerank request
reaching a daemon whose backend cannot serve it (the socket should
not have been bound).

### The daemon owns pair formatting (ADR 0013)

Consumers send `query` and `documents` as semantic intent. The daemon
builds the token sequence, mirroring
`tools/server/server-common.cpp:1543` (`format_prompt_rerank`):

- If the model carries a `rerank` chat template
  (`llama_model_chat_template(model, "rerank")`), substitute
  `{query}` and `{document}` into it.
- Otherwise assemble `BOS query [EOS] [SEP] document [EOS]`, honouring
  `llama_vocab_get_add_bos` / `_add_eos` / `_add_sep`, with SEP
  standing in for a missing EOS.

This is squarely ADR 0013: a consumer that had to know whether its
reranker wants a template or a SEP-joined pair would be reimplementing
model-specific shaping, and every consumer would reimplement it
differently. It also means swapping rerankers is a config change, not
a change to every caller.

### ADR 0012 stands — no relaxation, no supersession

A reranker is a distinct model artefact, so a daemon serving both
generation and rerank would hold two warm models. ADR 0017 already
answered this exact question for embeddings and **rejected** relaxing
ADR 0012; the reasoning transfers verbatim ("ADR 0012's rule isn't
about *what* the model does; it's about *how many* model artefacts
the daemon holds resident"). Operators wanting retrieval + rerank +
generation run three inferd processes on three sockets, the way they
would run three redis instances.

So this ADR supersedes nothing. Task #171 anticipated a "second
exception to ADR 0012" being needed; on inspection there is no first
exception to extend — ADR 0017 declined to make one.

The trait's capability flags are independent, so a single model
natively supporting both embed and rerank pooling would bind both
sockets. No shipped model does; the design permits it.

### Opt-in, off by default

`rerank: false` in the llamacpp config, matching `embed`'s existing
default. A rerank context is a second `llama_context` allocation
against the model plus its KV cache; deployments doing no retrieval
should not pay for it. Turning it on is a config edit, and the socket
appears because the capability flips — no separate socket toggle,
which would let the two disagree.

**This is deliberately a runtime flag, not a cargo feature.** Rerank
adds no dependency and no new attack surface: it is a pooling enum
value and ~200 lines against FFI that is already linked. A build flag
would double the artifact matrix to prove nothing, and an operator
who wanted rerank would need a different binary rather than a config
line. Contrast ADR 0028, where a build flag *is* correct because the
thing being removed is a dependency tree and an egress path — a
distinction worth naming, because "make it a feature flag" is not
uniformly the right answer.

### Scope

In:

- `inferd-proto`: `rerank/` module — `RerankRequest`, `RerankResolved`,
  `RerankResponse`, `RerankResult`, `RerankErrorCode`, `RerankUsage`,
  and the two bounds constants.
- `inferd-engine`: `Backend::rerank` with a default impl returning
  `Unsupported`; `RerankResult` / `RerankError`;
  `capabilities().rerank`; the llamacpp implementation.
- `inferd-daemon`: `lifecycle_rerank.rs`, `Router::dispatch_rerank`,
  socket binding, config plumbing.
- `inferd-client`: `RerankClient`.
- Tier 1 unit tests, Tier 2 mock-backend daemon tests, a Tier 3
  real-model binary.

Out, explicitly:

- **A default reranker model.** Model choice, licence review, and CAS
  manifest are a separate decision from the surface; the surface is
  useless without a model but freezing a wire shape does not require
  picking one. No auto-select entry (ADR 0023) until one is chosen.
- `/v1/rerank`-shaped HTTP in `inferd-http`. Ecosystem extension, and
  there is no OpenAI-standard rerank endpoint to be compatible with
  (Cohere's and Jina's shapes differ). Deferred until a consumer needs
  it.
- Cloud adapter rerank (`openai-compat`, `bedrock-invoke`) — they
  return `rerank_unsupported`.
- Cross-backend rerank routing beyond the capability filter
  `dispatch_rerank` applies.

## Consequences

**Why this is the right shape:**

- **No new protocol concepts.** Fourth socket, same NDJSON, same cap,
  same envelope. A consumer that speaks embed speaks rerank after
  reading one struct.
- **No new dependency and no bindgen change.** The engine path is a
  pooling enum value on machinery that already ships.
- **The expensive axis is bounded at the boundary.** `documents` is
  capped in the parser, where rejection is free.
- **Capability-driven binding keeps the surface honest.** Dialling
  rerank on a generate-only daemon is connection-refused, not a
  timeout or a runtime error.
- **Consumers stay out of tokenizer business.** Template-vs-SEP is a
  model detail the daemon owns.

**What this costs:**

- A fourth frozen wire surface. Additive optional fields only, from
  the moment it ships.
- The `Backend` trait grows a fourth method. Defaulted, so no adapter
  outside llamacpp changes.
- Operators wanting rerank + generation run two processes — the cost
  ADR 0012 and ADR 0017 already accepted.
- Rerank latency is `O(documents)` forward passes and holds the shared
  admission permit throughout. A 256-document rerank is a long
  request; it does not stream, so there is no partial progress. That
  is inherent to cross-encoding, and the document cap is what keeps
  it bounded.
- One more socket in `doctor` output and the relay's surface list
  (ADR 0024).

**What this explicitly does not change:**

- ADR 0006 — no HTTP in the daemon.
- ADR 0007 — no in-daemon retry; caller retries.
- ADR 0012 — one warm model per process, unrelaxed.
- ADR 0017 — embed envelope untouched; rerank is its sibling.
- ADR 0021 — v2 generation wire untouched; `wire_version` unmoved.

## Alternatives considered

- **A `rerank: true` flag on the embed request.** Rejected. The embed
  envelope is frozen, so this could only arrive as an optional field
  that changes the response type — and it conflates a vector-returning
  surface with a score-returning one in one parser. Same reasoning
  ADR 0017 used to decline putting embed behind a flag on v1/v2.
- **Return sorted documents instead of indices.** Rejected. Echoes up
  to 8 MiB the caller already has, for no added information.
- **Let consumers format the pair and send pre-joined strings.**
  Rejected on ADR 0013 grounds: it exports tokenizer and
  template-detection logic to every consumer, each of which would get
  it subtly differently, and it would make swapping rerankers a
  change to every caller.
- **Normalise scores to 0..1 across models.** Rejected. Would require
  knowing each model's output distribution; a sigmoid over logits is
  a guess that makes incomparable numbers *look* comparable. Better to
  document that scores are model-specific and ordinal.
- **Make rerank a build feature like airgapped (ADR 0028).**
  Rejected. It adds no dependency and no egress; a feature flag would
  double the artifact matrix and force a rebuild for a config change.
  Build flags are for removing dependency trees, not for gating code
  paths.
- **Reuse the generation socket with a `task: "rerank"` field.**
  Rejected. The generation wire is length-prefixed and type-tagged for
  streaming token frames (ADR 0021); rerank is single-frame NDJSON
  with no streaming concept.
- **Defer rerank until a consumer asks.** Considered seriously. The
  surface ships without a default model, so nothing is immediately
  usable — but the wire shape is small, the engine path is proven, and
  freezing it now while embed's shape is fresh keeps the two siblings
  consistent. Picking the model is the follow-up.

## References

- ADR 0006 — lean core; HTTP stays out of the daemon.
- ADR 0007 — routing and failure semantics, reused unchanged.
- ADR 0012 — one warm model per process; not relaxed here.
- ADR 0013 — daemon owns model-specific shaping (pair formatting).
- ADR 0017 — the embed surface this mirrors, including its rejection
  of an ADR 0012 exception.
- ADR 0023 — boot-time auto-select; no rerank entry until a default
  model is chosen.
- ADR 0028 — the airgapped build feature, and why *that* one is a
  build flag while this one is not.
- `THREAT_MODEL.md` F-1 — the attachment-amplification precedent
  behind bounding `documents` at the proto layer.
- `vendor/llama.cpp/include/llama.h:177,576,1030` —
  `LLAMA_POOLING_TYPE_RANK`, `llama_model_n_cls_out`,
  `llama_get_embeddings_seq` rank semantics.
- `vendor/llama.cpp/common/common.cpp:1373` — vocab preconditions for
  reranking.
- `vendor/llama.cpp/tools/server/server-common.cpp:1543` —
  `format_prompt_rerank`, the pair-formatting reference.
- Issue #171 — the request.
