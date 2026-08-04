# 0026. Chat rendering is a registry keyed to model family, not a hardcoded Gemma renderer

- Status: accepted
- Date: 2026-08-04

## Context

ADR 0013 made the daemon the gateway: it owns the model-specific
shaping that turns a semantic `ResolvedV2` into what the engine
consumes. For the llama.cpp backend that shaping is a flat prompt
string carrying the model's control tokens, plus an ordered slice of
attachments for `mtmd_tokenize` to splice bitmaps into.

That responsibility is implemented for exactly one model family.
`crates/inferd-engine/src/llamacpp/backend.rs:583` reads:

```rust
let renderer = Gemma4Renderer::new();
```

There is no dispatch. `Gemma4Renderer` emits Gemma 4's control tokens
unconditionally — `<bos>`, `<|turn>system\n`, `<|think|>`,
`<turn|>\n`, `<|turn>model\n`, `<|tool_call>call:`, `<|tool_response>`
(`chat_template/gemma4.rs:150,178,180,183,205,270,290`). Nothing in
the crate reads the loaded model's own `tokenizer.chat_template`.

This is already a live defect, not a theoretical gap.
`crates/inferd-daemon/src/config.rs:117` exposes
`pub model_path: Option<PathBuf>`, so an operator can point the daemon
at any GGUF in the store today. A non-Gemma model loads successfully,
reports ready, binds the generation socket, and then answers every
request with its prompt wrapped in another model's turn markers. The
model does not error — it produces fluent, confidently wrong output.
This is the same failure shape as the audio sample-rate bug ADR 0025
addressed: an undetectable wrong answer is worse than a loud failure.

The immediate driver is `ibm-granite/granite-docling-258M`. The
vendored llama.cpp `b9850` supports it end to end — the vision half
converts via `conversion/__init__.py:273`
(`Idefics3ForConditionalGeneration` → `smolvlm`) and runs as
`MTMD_SLICE_TMPL_IDEFICS3` (`tools/mtmd/mtmd.cpp:222`); the text half
is `LlamaForCausalLM` via `conversion/__init__.py:137`. The engine can
run it. inferd cannot, and the *only* reason is the hardcoded
renderer.

Three constraints shape the answer:

1. **`general.architecture` is not a sufficient key.** granite-docling's
   text tower converts to GGUF architecture `"llama"`
   (`gguf-py/gguf/constants.py:990`) — the identical string a
   Llama-3-Instruct GGUF reports. Their prompt formats share nothing.
   Architecture identifies the *tensor topology* the engine needs; it
   does not identify the *prompt grammar* the renderer needs. Keying a
   renderer registry on it would reintroduce the exact
   silently-wrong-output bug for a different pair of models.

2. **llama.cpp cannot render arbitrary templates for us.**
   `llama_chat_apply_template` (`include/llama.h:1186`) explicitly
   "does not use a jinja parser. It only support a pre-defined list of
   templates" (`:1178`). Delegating is not an option for a model
   outside that list. `llama_model_chat_template()` (`:614`) returns
   the raw jinja source, but reading it is not rendering it.

3. **The media path is already model-agnostic and must stay that way.**
   `MEDIA_MARKER` (`chat_template/gemma4.rs:57`) is mtmd's *default*
   marker, not a Gemma token; the renderer places `<__media__>` and
   `mtmd_tokenize` substitutes the right per-model image/audio
   embedding, including the Idefics3 slice template. So attachment
   handling generalises for free. Only the surrounding text grammar is
   per-family.

## Decision

**Replace the hardcoded renderer with a registry of hand-written
renderers keyed to model family, resolved once at model load, and fail
loudly at load time when no renderer matches.**

Four parts:

**1. A `ChatRenderer` trait.** The existing `Gemma4Renderer::render`
signature is already the right seam — it takes `&ResolvedV2` and
returns a prompt string plus an ordered attachment slice
(`chat_template/gemma4.rs:118`, `Gemma4Rendered` at `:67`). That
becomes the trait; `Gemma4Renderer` becomes its first implementor,
unchanged in behaviour. `MEDIA_MARKER` and the error type move to the
module root as shared surface.

**2. A `family` identifier, resolved at load, not per request.** The
resolution order:

- An explicit `chat_template` field in the backend's config entry.
  Operator-declared, always wins, never guessed.
- Otherwise, detection from GGUF metadata read via the *existing*
  `read_gguf_meta_string` helper (`backend.rs:483` — already general,
  already used for `general.name` at `:467`; no new FFI is needed).
  Detection matches on `general.architecture` **together with** a
  fingerprint of `tokenizer.chat_template`, precisely because
  constraint (1) makes architecture alone ambiguous.
- Otherwise: **fail the model load.** Not a fallback to Gemma.

**3. Fail loud on unknown.** A GGUF whose family cannot be resolved
aborts backend init with a message naming the architecture, the
detected template fingerprint, and the `chat_template` config field
the operator can set. Per invariant #5 and ADR 0009, the generation
socket is never bound, so no consumer can reach a
mis-rendering daemon. The daemon logs the failure to the activity log
before exit, as backend-init failures already do since v0.6.0.

**4. A `docling` renderer as the second implementor**, proving the
seam with a real second family rather than a hypothetical one.

The wire does not change. `wire_version` does not move. Rendering is
entirely daemon-internal — a consumer sends the same semantic
`messages[]` regardless of which family serves it, which is the whole
point of ADR 0013.

## Consequences

### Why this is the right shape

- **It closes a correctness hole, not just a feature gap.** The
  headline benefit is not "docling runs"; it is that pointing
  `model_path` at an unsupported GGUF stops producing plausible
  garbage. A daemon that refuses to start is debuggable in one line of
  log. A daemon that answers fluently in the wrong grammar costs a
  day.
- **It is squarely ADR 0013's remit.** Per-family prompt grammar is
  engine-facing shaping, the thing ADR 0013 explicitly assigns to the
  daemon. This adds no consumer-facing surface, no knowledge of
  application logic, and nothing middleware could reasonably own — a
  consumer cannot render Gemma turn markers without linking the
  engine's tokenizer, which is why this cannot live outside.
- **Detection is a convenience; declaration is the contract.** The
  config field means an operator with an exotic GGUF is never blocked
  on inferd shipping a detector, and detection failures degrade to an
  error plus a documented knob rather than to a wrong answer.
- **It reuses the existing seam.** `render()` already returns exactly
  what the backend needs, and nothing downstream of it in
  `generate_v2` is Gemma-specific — attachments, the audio-rate check,
  grammar, and the tool parser all operate on family-neutral data.
  This is a trait extraction, not a rewrite.
- **It is a prerequisite for anything multi-model.** A daemon holding
  two generate models needs a renderer per model as a matter of
  arithmetic. Landing this first keeps that decision independent of
  this one.

### What this costs

- **A hand-written renderer per family, forever.** Each new family is
  real work — read the model's jinja template, transcribe its grammar,
  test it. This is the deliberate trade against a jinja engine
  (below). It bounds how fast inferd can adopt new families, and that
  bound is accepted.
- **Detection is a heuristic and will occasionally be wrong.**
  Mitigated by direction: it errs toward *refusing to start*, never
  toward rendering with a guessed family. The explicit config field is
  the escape hatch.
- **One behaviour change for existing installs.** A daemon currently
  pointed at a non-Gemma GGUF "works" (wrongly) and will now fail to
  start. That is the fix, not a regression, but it is a breaking
  behaviour change for anyone who had that configuration and has to be
  called out in `CHANGELOG.md` under Changed.
- **The tool-call parser is family-coupled too.** `tool_parser.rs`
  parses Gemma's `<|tool_call>` shape. A family whose tool grammar
  differs needs a parser alongside its renderer. This ADR scopes the
  registry to rendering; the parser generalises the same way when a
  family that needs it arrives. Docling does not — it emits DocTags,
  not tool calls.

### What this explicitly does not change

- **No wire change.** Both live surfaces stay frozen; no new field, no
  `wire_version` bump. Family is daemon-side configuration.
- **No `model` field on the wire.** ADR 0012 still stands here. This
  ADR gives one process one renderer for its one generate model.
- **No media-path change.** `MEDIA_MARKER` stays mtmd's default and
  attachment ordering is untouched, per constraint (3).
- **No new dependency.** Detection uses the FFI helper already in the
  tree. Nothing is added to the daemon's link surface.

## Alternatives considered

- **Embed a jinja engine and render the GGUF's own
  `tokenizer.chat_template`.** This is what llama.cpp's server does
  (via minja), and it is genuinely tempting: the model's own template
  is the authoritative source, so it would support every family with
  no per-family work. **Rejected on trust-boundary grounds.** The
  template is arbitrary program text carried inside a file, and
  `model_path` accepts any GGUF in the store. inferd's fetch path is
  deliberately one-URL-one-SHA (ADR 0010), but a store blob may have
  arrived from any tool sharing the CAS convention (ADR 0011).
  Evaluating jinja from that input inside the daemon adds a template
  interpreter — with its own CVE stream — to the most privileged
  process on the host, to save writing renderers. It also does not
  fully solve the problem: templates emit model-specific image tokens
  (`<|image_pad|>`, `<image>`), so a marker-substitution mapping per
  family would still be needed, and the per-family work does not
  actually go to zero. Revisit only with a sandboxed,
  non-Turing-complete evaluator and a signed-template story.
- **Key the registry on `general.architecture` alone.** Rejected on
  constraint (1) — `"llama"` is shared by granite-docling's text tower
  and Llama-3-Instruct, so this reintroduces the silently-wrong-output
  bug it was meant to fix.
- **Delegate to `llama_chat_apply_template`.** Rejected on constraint
  (2): predefined list only, no jinja, and it has no concept of the
  attachment ordering the mtmd path requires.
- **Keep the hardcoded renderer and simply reject non-Gemma models at
  load.** This is the minimal fix for the correctness hole and was
  seriously considered, since it is a fraction of the work. Rejected
  because it permanently forecloses the second family — and the
  driver here is a concrete model that the vendored engine already
  supports. The fail-loud behaviour is a *component* of this ADR, not
  an alternative to it.
- **Put rendering in the consumer.** Rejected: it inverts ADR 0013,
  forces every consumer to link a tokenizer and track model-specific
  control tokens, and would make the wire model-specific — exactly
  what typed content blocks (ADR 0015) exist to avoid.

## References

- ADR 0013 — inferd is the gateway, not the pipe (this is a direct
  application; per-family prompt grammar is the daemon's job).
- ADR 0015 / 0021 — the v2 generation wire stays semantic and frozen;
  rendering is downstream of it.
- ADR 0016 / 0025 — the same "reject rather than silently produce a
  fluent wrong answer" principle, applied to media rate conversion.
  This ADR applies it to prompt grammar.
- ADR 0009 — backend readiness gates socket binding, which is what
  makes fail-loud safe.
- ADR 0012 — one warm model per process; unchanged by this ADR.
- ADR 0010 / 0011 — the model provenance argument behind rejecting an
  in-daemon jinja evaluator.
- `crates/inferd-engine/src/llamacpp/backend.rs:583` — the hardcoded
  call site this ADR replaces; `:483` — the metadata helper it reuses.
- `crates/inferd-daemon/src/config.rs:117` — `model_path`, which makes
  the current gap reachable.
