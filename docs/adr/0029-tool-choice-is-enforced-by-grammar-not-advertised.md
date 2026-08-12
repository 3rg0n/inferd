# 0029. `tool_choice` is enforced by grammar, not advertised as a hint

- Status: accepted
- Date: 2026-08-09
- Erratum: 2026-08-09, `f5c9d2c` — see *Erratum* below

## Erratum — corrected in place, 2026-08-09 (`f5c9d2c`)

ADRs here are immutable once accepted (`docs/adr/README.md`), and this
one was edited after acceptance. Recording that plainly rather than
leaving it to `git log`.

As first accepted (`5f0480f`), the *Good* bullet claimed the Tier-3
test prompts "do not use any tools, just say hi" **and a call comes
back anyway**. A 600-token re-run refuted it: the model loops a
hallucinated `<execute_tool>{…}` and terminates on `MaxTokens` again,
so the failure mode is degenerate repetition, not an insufficient
budget. No grammar of this shape delivers "a call arrives", and the
test asserting it was a test that fails on correct code.

The **decision did not change** — not the field, the wire, the
grammars, or any rejection path. What changed is the claim about what
the decision *guarantees*: `required` makes *ending the turn* without
a call unreachable, which is narrower than the mode's name suggests.

Corrected in place rather than superseded by an ADR 0030 because the
choice being recorded is unchanged, and superseding would split one
live decision across two documents where the second says only "the
first cited its evidence wrongly" — the audit trail immutability
exists to protect is better served by this note than by that. The
window matters too: both versions landed the same day, before any
release tag, so no deployed consumer read the wrong sentence. A
correction discovered after a tag ships gets an ADR 0030, not an
erratum.

## Context

The v2 wire has carried a `tools[]` table since ADR 0015, but nothing
that says whether the model *may*, *must*, or *must not* use it.
Consumers therefore express the intent in prose ("you must call a
tool"), which is exactly the thing a language model is free to
ignore. Issue #38 asks for `tool_choice`.

Every provider surface has the field, so the wire half is
uncontroversial: OpenAI spells the modes `auto` / `required` / `none`
plus an object naming one function; Anthropic spells them `auto` /
`any` / `none` plus `tool`. Adding an optional string to `RequestV2`
is additive and needs no `wire_version` bump.

The hard part is what the field *means* on the local backend. Two
shapes were available:

1. **Advisory.** Put the mode in the rendered prompt and let the model
   comply or not. Cheap, and it is what upstream `llama.cpp` does for
   `none` (it builds no grammar at all).
2. **Enforced.** Compile the loaded family's tool-call syntax to GBNF
   and install it on the sampler, so non-compliance is not a reachable
   sampling path.

An advisory `tool_choice` is worse than no field, and that is the
whole decision. A caller that sets `required` and gets prose has been
handed a *silent* failure: the field's existence is a promise that it
does not have to write the retry. This is the same fail-open class
ADR 0025 refuses for audio sample rates — a fluent wrong answer
instead of a detectable error.

Three sub-questions follow from choosing enforcement.

1. **Can the grammar be generated from the tool schemas?** Issue #38
   proposes reusing `json_schema_to_gbnf`. It cannot: Gemma 4's
   tool-call syntax is not JSON. It is
   `<|tool_call>call:NAME{KEY:<|"|>VALUE<|"|>,…}<tool_call|>`, so the
   grammar must be hand-written per family.
2. **What happens when `response_format` is also set?** There is one
   grammar sampler. Upstream early-returns on `has_response_format`
   and silently drops the tool constraint.
3. **Can a family that cannot enforce it fall back to advisory?** That
   is question 1 restated as an inheritance default.

## Decision

**`tool_choice` is a constraint on every backend that accepts it, and
a backend that cannot enforce it rejects the request.**

Concretely:

- **Wire (`inferd-proto`).** `RequestV2.tool_choice: Option<ToolChoice>`,
  serialised as a bare string (`"auto"` / `"required"` / `"none"`),
  omitted when absent. An unrecognised value parses to
  `ToolChoice::Unknown` for forward compatibility and `resolve()` then
  rejects it, rather than guessing which mode was meant. `resolve()`
  also rejects a `tool_choice` sent with no `tools` — the field
  constrains that table, and with nothing to constrain the request is
  a mistake, not a no-op. Additive; `wire_version` does not move.

- **Naming a specific tool is not modelled.** OpenAI's
  `{"type":"function","function":{"name":…}}` is *rejected* by the
  bridge with a 400, not widened to `required`. Widening would let the
  model call a different declared tool while the caller believed it
  had pinned one — a fail-open dressed as compatibility. The
  documented workaround is `required` with only that tool declared,
  which is the same guarantee expressed in what the wire can say.
  Adding a named mode later stays additive.

- **Enforcement is the renderer's job** (ADR 0026 registry). The
  `ChatRenderer` trait gains `tool_call_grammar(choice, tools) ->
  Result<Option<ToolGrammar>, RenderError>`, where `ToolGrammar`
  carries GBNF text plus optional lazy trigger patterns. **The default
  implementation refuses every mode** with `RenderError::Unsupported`,
  so a family opts in deliberately; inheriting a silently-unenforced
  `required` is precisely what this ADR exists to prevent. Gemma 4 is
  the only family that opts in at this ADR's date.

- **Per-mode grammar shape** (Gemma 4):
  - `required` → **eager** grammar whose root demands one complete
    call. `llama_grammar_apply_impl` masks every end-of-generation
    token while no stack is empty, so ending the turn with prose is
    not a reachable path. This is the mechanism the whole decision
    rests on.
  - `auto` → **lazy** grammar armed on `<|tool_call>`. It cannot force
    a call (that is `required`'s job) but once the model starts one,
    the body syntax is pinned — which removes a pre-existing failure
    where a malformed body aborted an otherwise good generation.
  - `none` → grammar excluding the opener **as text**, not as a token
    id. A `!<|tool_call>` token rule is fail-open: `<`, `|tool`,
    `_call>` spells the same opener in ordinary pieces, and inferd's
    parser scans detokenised text, so it would fire on the spelled-out
    form. This is a deliberate divergence from upstream, which builds
    no grammar for `none`.

- **`response_format` + `tool_choice` is rejected** with
  `invalid_request`. Only one grammar can be installed, so honouring
  either silently drops the other. We diverge from upstream here on
  purpose: upstream drops the *tool* constraint, which is the exact
  fail-open the field exists to close. Nothing regresses, because
  `tool_choice` is new — no caller can have depended on the pair
  working.

- **Cloud adapters forward, never drop.** `openai-compat` sends the
  three modes verbatim; `bedrock-invoke` maps `required` → Anthropic's
  `any`. Both **error** on a value they cannot express rather than
  omitting the field, because omission would leave a `required`
  request best-effort upstream while the caller believed it held a
  guarantee.

- **Scope limit: only tool *names* are masked, not argument schemas.**
  The grammar pins the call syntax and constrains the name to the
  declared table; argument values follow the family's value grammar,
  not each tool's `input_schema`. Upstream carries the same limitation
  as a live TODO. Per-tool argument masking is additive and does not
  change any decision above.

## Consequences

**Good.**

- `required` is a real guarantee on the llamacpp backend, verifiable:
  the Tier-3 test prompts "do not use any tools, just say hi" and the
  model cannot end its turn without a call — it runs to `MaxTokens`
  instead. An advisory implementation returns a cheerful "Hi." and
  `EndTurn`, which is the outcome the grammar makes unreachable. See
  the matching entry under *Costs* for what this does **not**
  guarantee; the distinction was established by running the test, not
  by reasoning about the grammar.
- `auto` fixes a latent bug for free — a model-emitted malformed call
  body used to abort the generation, and the lazy grammar makes it
  unreachable.
- A new family cannot accidentally ship a fake guarantee: the trait
  default refuses, so silence is a 400, not a lie.

**Costs, accepted.**

- **Per-family hand-written GBNF.** Every family that wants
  `tool_choice` writes and tests its own grammar; there is no generic
  path, because the syntax is not JSON. Families that skip it reject
  the field.
- **The pair rejection is a real capability gap.** "Emit a tool call
  *and* conform to a JSON schema" is unexpressible until llama.cpp can
  hold two grammars, or until the two are composed into one. Rejecting
  is the honest report of that gap.
- **Argument values are unconstrained beyond syntax.** A model can
  emit a well-formed call with arguments the tool's schema would
  reject. The caller still validates arguments.
- **`required` guarantees the turn cannot *end* without a call — not
  that a call arrives.** This is the limit of what a grammar of this
  shape can promise, and it is narrower than the mode's name suggests.
  The eager root is `prefix tool-call` with every `prefix` state
  nullable, so unlimited non-opener text is legal; a model that
  disagrees with the instruction can decline for as long as its budget
  allows. `EndTurn` with no call is unreachable, which is the silent
  failure the field exists to close. `MaxTokens` with no call is not.

  Measured on the Tier-3 adversarial prompt ("Do not use any tools.
  Just say hi.") against Gemma 4 E4B: the model loops a *hallucinated*
  `<execute_tool>{…}` — never the real `<|tool_call>` opener, which the
  grammar does bar — argues with itself in the thinking channel, and
  terminates on `MaxTokens`. Raising the budget from 128 to 600 tokens
  changed nothing but the length of the loop: the failure mode is
  degenerate repetition, not insufficient room.

  The prefix is not removable. A root of bare `tool-call` would mask
  the `<` that opens Gemma's `<|channel>thought` block and force the
  model to call blind. Upstream has the identical structure and the
  identical weakness — `scan_to_toolcall = p.until("<|tool_call>")`
  followed by `repeat(min=1)` in `common_chat_params_init_gemma4`.

  So a `required` caller must treat `MaxTokens` as "no call arrived"
  rather than assuming one did. That outcome is self-announcing, which
  is the property that matters: the caller can detect it and retry,
  where an advisory implementation's plausible prose is undetectable.
- **Narrowings versus upstream's grammar** (identifier-shaped dict
  keys; no `"` or `\` in string content) exist because inferd parses
  these bodies with its own parser rather than upstream's PEG, and a
  grammar wider than its own parser admits output the parser then
  rejects as `Malformed`. Neither loses an argument that is currently
  representable on this wire.

## References

- Issue #38 (the request; its `json_schema_to_gbnf` proposal is
  superseded by the hand-written-per-family decision above).
- ADR 0015 — typed content blocks, where `tools[]` arrived.
- ADR 0026 — the chat-renderer registry this hangs the grammar off.
- ADR 0013 — inferd is the gateway, not the pipe: engine-level
  shaping (which this is) is squarely a daemon concern.
- ADR 0025 — the fail-open precedent: reject rather than silently
  produce a fluent wrong answer.
- `vendor/llama.cpp/src/llama-grammar.cpp` —
  `llama_grammar_apply_impl` (EOG masking; the enforcement mechanism)
  and `llama_grammar_accept_impl` (lazy trigger replay).
- `vendor/llama.cpp/common/chat.cpp` —
  `common_chat_params_init_gemma4` (the syntax transposed here) and
  the `has_response_format` early return we diverge from.
