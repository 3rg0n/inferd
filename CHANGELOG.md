# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **An unpaired `tool_result` is now rejected instead of guessed at.** A
  `tool_result` whose `tool_call_id` matches no `tool_use` earlier in the
  request fails with `invalid_request`, naming the id. Previously the
  Gemma 4 renderer inferred the tool name when `tools[]` had exactly one
  entry, and emitted unlabelled content when it couldn't — so a result
  could reach the model *attributed to a tool that was never called*, or
  to nothing at all. The model reads either as fact and no consumer can
  detect it downstream, which is the fail-open class ADR 0025 refuses.

  inferd sits between middleware and model and relays; pairing results to
  calls is the middleware's bookkeeping and `tool_call_id` is how it
  states the answer, so an id that resolves to nothing is the caller's
  bug and is reported as one. Granite already refused these outright.

  **Migration:** a caller replaying a tool conversation must include the
  `tool_use` blocks, not only the `tool_result`s. Callers that already
  send matching ids — the documented shape — are unaffected.

### Fixed

- **`docs/consuming-across-a-boundary.md` advertised two `inferd-http`
  surfaces where one exists.** ADR 0020 sketched a *Surface B* — inferd's
  native frames over a localhost port — and the boundary guide listed it
  as something you could use. It was never built, and ADR 0024 removed its
  motivation: a shared first-party relay collapses every consumer into one
  peer identity at the daemon, so cross-boundary bridging belongs to the
  consumer. The guide now says so, and points at Option C (roll your own)
  for native frames. The same passage claimed TLS terminates at the
  bridge; the bridge links no TLS stack and speaks plain HTTP behind a
  reverse proxy, which its own README already stated correctly.

### Added

- **`tool_choice` on the v2 generation wire — enforced by grammar, not
  advertised as a hint** (issue #38, [ADR 0029](docs/adr/0029-tool-choice-is-enforced-by-grammar-not-advertised.md)).
  `RequestV2.tool_choice` accepts `"auto"` / `"required"` / `"none"` as a
  bare string, omitted when absent. Additive: `wire_version` does not move
  and a v0.7.0 client interoperates unchanged.

  The point of the field is that it is a **constraint**. On the llamacpp
  backend the loaded family's tool-call syntax is compiled to GBNF and
  installed on the sampler, so `required` is not a request the model can
  decline: the eager grammar's root demands a complete call, and
  `llama_grammar_apply_impl` masks every end-of-generation token while no
  stack is empty — *ending the turn* with prose is not a reachable sampling
  path. The Tier-3 test proves it adversarially against a real Gemma 4,
  prompting "do not use any tools, just say hi": the model cannot finish,
  and runs to `max_tokens` instead. An advisory implementation returns a
  cheerful "Hi." and `end_turn`.

  What `required` does **not** promise is that a call ever arrives. The
  eager root is `prefix tool-call` with every prefix state nullable, so a
  model that disagrees with the instruction can decline until its budget
  runs out — measured, and unchanged by raising the budget, because the
  failure mode is degenerate repetition rather than insufficient room.
  Upstream carries the identical structure and weakness. Callers branch on
  `stop_reason`: `max_tokens` with no `tool_use` block means no call
  arrived. That is self-announcing and therefore retryable, which is the
  whole difference from an advisory field's plausible prose.

  `auto` installs a *lazy* grammar armed on `<|tool_call>`. It cannot force
  a call, but once the model starts one the body syntax is pinned — which
  fixes a pre-existing failure for free, where a model-emitted malformed
  call body aborted an otherwise good generation. `none` excludes the
  opener **as text** rather than as a token id: a `!<|tool_call>` token
  rule is fail-open, because `<`, `|tool`, `_call>` spells the same opener
  in ordinary pieces and inferd's parser scans detokenised text. Upstream
  llama.cpp builds no grammar for `none` at all; this is a deliberate
  divergence.

  Enforcement hangs off the ADR 0026 renderer registry as
  `ChatRenderer::tool_call_grammar`, whose **default implementation
  refuses every mode**. A family opts in deliberately; inheriting a
  silently-unenforced `required` is exactly the failure the field exists to
  close. Gemma 4 is the one family that opts in — its call syntax is not
  JSON, so `json_schema_to_gbnf` cannot express it and the grammar is
  hand-written, which is why #38's own proposal to reuse the JSON-Schema
  path is superseded.

  Surfaces: `inferd-http` maps OpenAI's three string forms and returns
  **400** for anything else — including the named-function form
  `{"type":"function","function":{"name":…}}`, which is rejected rather
  than widened to `required` (widening would let the model call a
  *different* declared tool while the caller believed it had pinned one;
  the workaround is `required` with only that tool declared). The cloud
  adapters forward rather than drop — `openai-compat` sends the modes
  verbatim, `bedrock-invoke` maps `required` to Anthropic's `any` — and
  both error on a value they cannot express, because omitting the field
  would leave a `required` request best-effort upstream while the caller
  believed it held a guarantee. `clients/go` gains `ToolChoice` plus the
  three constants.

  Two deliberate rejections, both `invalid_request`: a `tool_choice` with
  no `tools` (there is nothing to constrain), and `response_format`
  together with `tool_choice` — only one grammar can be installed, so
  honouring either silently drops the other. Upstream drops the *tool*
  constraint in that case, which is the precise fail-open this field
  exists to close; nothing regresses from refusing, since `tool_choice` is
  new. Scope limit, matching upstream's own live TODO: enforcement pins
  call syntax and masks tool *names* to the declared table, but argument
  values are not constrained by each tool's `input_schema` — callers
  still validate arguments.

### Validation

- **macOS arm64 Metal — airgapped *archive* install=work, all 5 checklist
  items green, no new defects (2026-08-08):** completes issue #56's
  cross-platform matrix at 3 of 3 platforms (Windows CUDA + Linux/WSL2 CUDA
  already green), **closing #60 and #56**.
  Ran against the published `inferd-airgapped-v0.7.0-aarch64-apple-darwin.tar.gz`,
  both archives extracted at real default paths. `install-launchagent.sh`
  correctly detects the airgapped profile and points at `inferdctl import`,
  not `watch`; `--version` works pre-install with `backends/` unflattened
  (dyld's lazy `@rpath` resolution, same shape as Linux's ELF `$ORIGIN` —
  Windows's `0xC0000135` silent-failure trap is confirmed Windows-specific).
  `inferdctl import` lands a model with correct CAS layout/manifest/SHA.
  Real generate (`"AIRGAPPED070"`, `backend=llamacpp`) + real embed
  (256-dim, L2=1.0) from the installed daemon; socket modes match
  invariant #6. `inferd-http` from the airgapped archive — third platform
  for this item — serves real chat (non-stream + SSE) and embeddings with
  no addr overrides. Accelerator selection unaffected: airgapped and
  networked archives both log `chosen="metal"` on the same box. TLS
  control (`grep -a -o | wc -l`, anchored on crate-path patterns): 0 hits
  for `rustls-`/`webpki`/`/ureq-`/`ring-0.` in the airgapped binary vs.
  45/32/7/30 in the networked control. #57 and #51 both confirmed
  cross-platform (not macOS-specific); **#58's macOS fix independently
  verified on real hardware** — both the "CLI on PATH" and "CLI not on
  PATH" branches print a resolvable `inferdctl` invocation, closing the
  "unverified on hardware" note left when the fix landed. See
  `docs/v0.6-validation.md`, issues #56/#60.

### Fixed

- **`inferdctl status` dropped four capabilities fields that `doctor`
  printed, leaving the scriptable surface the less informative of the two**
  (`crates/inferd/src/main.rs`, issue #61). `admin_event_to_json` rebuilds
  its JSON from the typed `AdminEvent` rather than relaying the raw admin
  line — deliberately, so the CLI's output tracks the spec rather than the
  daemon's encoder choices — but that only works if it stays complete, and
  it had no arms for `wire_version`, `audio_sample_rate`, `device_name` or
  `vram_total_bytes`. All four are carried by the typed view and sent by
  the daemon; `doctor` printed `wire_version` on its `backend:` line and
  the other two device fields on a `device:` line, so `status | jq` — the
  machine-readable path — saw strictly less than the human punch list.

  `audio_sample_rate` was the one that cost real time: it is the rate audio
  attachments must already be at, since the daemon rejects any other rather
  than resampling (ADR 0016 / ADR 0025), and it was not printed by
  `doctor` either — so the required rate was unreachable from the CLI
  entirely, discoverable only from a raw admin-socket read or
  `inferdctl watch`. It now appears in `status`'s JSON and, next to
  `audio=` on `doctor`'s backend line, appended rather than columned so a
  backend that ingests no audio prints nothing instead of a meaningless
  `audio_sample_rate=0`.

  Additive to the CLI's output, not to any wire surface: the daemon already
  sent all four. The guard against a repeat is compile-time — the new
  `admin_json_covers_every_capabilities_field` test destructures
  `AdminEvent` exhaustively, so adding a field to it without a matching arm
  in the renderer now fails to build (verified by planting a field:
  `E0027 pattern does not mention field`). Four tests in total, and
  `doctor`'s backend line moved into `doctor_backend_line` so its field
  coverage is assertable without a live daemon — the same extraction
  `render_status` got for #57. Verified live on Windows x86_64 CUDA against
  a current daemon: `status` now emits `wire_version=1`,
  `device_name=CUDA0`, `vram_total_bytes=17094475776` on both backends and
  `audio_sample_rate=16000` on the audio-capable one only, matching
  `doctor` field for field.

- **`gpu_layers` reported `0` — "CPU-only" — on a fully-offloaded GPU**
  (`crates/inferd-engine/src/llamacpp/backend.rs`, issue #51).
  `build_accelerator_info` clamped the configured `n_gpu_layers` with
  `.max(0)` before publishing it. But llama.cpp defines a **negative**
  `n_gpu_layers` as "offload all layers" (`llama.h`), and the shipped
  default config is `-1`, so the clamp mapped the value meaning *offload
  everything* onto the one value meaning *offload nothing*. Every stock
  GPU install therefore advertised `accelerator=cuda gpu_layers=0` over
  the admin surface and in `inferdctl doctor` — a self-contradicting line
  that reads as "GPU present, not being used". Filed against macOS Metal;
  it was never platform-specific, and was reproduced on Windows CUDA where
  generation was measurably running on the GPU while the report said
  otherwise.

  The adapter now computes the figure the way libllama computes it, which
  fixes a second lie in the same line found during review: a configured
  count *larger than the model* was also echoed back verbatim, so
  `n_gpu_layers: 999` reported 999 offloaded layers on a 42-layer model.
  libllama caps it — `act_gpu_layers = min(n_gpu_layers, n_layer_all + 1)`
  (`llama-model.cpp`) — and a negative value resolves to that same ceiling
  (`llama_model::n_gpu_layers()` → `hparams.n_layer_all + 1`).
  `n_layer_all` has no getter of its own, but it is recoverable from two
  public ones — `llama_model_n_layer()` returns `n_layer_all -
  n_layer_nextn`, so summing it with `llama_model_n_layer_nextn()` restores
  the total; the `+ 1` is libllama's own accounting for the output layer.
  A configured value below the ceiling still reports verbatim, because
  below the ceiling it *is* what gets offloaded. `0` keeps its existing
  meaning, which is exactly why `-1` could not be allowed to collide with
  it — a consumer reads `gpu_layers: 0` to learn the daemon is CPU-bound,
  and the ADR 0019 force-CPU escape hatch deliberately sets it. The one
  clause deliberately not mirrored is libllama's `devices.empty() ? 0`:
  a device-less host arrives as `AcceleratorKind::Cpu`, for which
  `LlamaCpp::new` already forces `0` before this code runs.

  Reporting-only: no wire change, no field added, and no change to what is
  actually offloaded — `load_model` always received the configured value
  untouched. Verified live on a two-backend CUDA daemon, which now reports
  `gpu_layers=43` for Gemma 4 E4B (42 blocks + output layer) and
  `gpu_layers=25` for EmbeddingGemma 300M, where both previously said `0`.
  Seven unit tests cover `-1`, other negatives (`llama.h` says *a* negative
  value, not specifically `-1`), a NextN model where reading only the first
  getter under-reports, configured `0` staying `0` without reading the
  model at all, a partial offload reported verbatim, an over-ask and
  `i32::MAX` capped at the ceiling, and absurd model-reported layer counts
  saturating rather than wrapping.

- **`inferdctl status` exited 1 against a healthy daemon and printed the
  wrong backend's capabilities** (`crates/inferd/src/main.rs`,
  `crates/inferd-client/src/admin.rs`, issue #57). `cmd_status` read a
  fixed *two* admin frames and looked for one whose `status` was not
  `capabilities` to decide readiness. But the daemon replays one
  `capabilities` frame **per registered backend** before the lifecycle
  snapshot (`admin::latest_capabilities`, name-sorted), so the shipped
  two-backend default (generation + embed) sends three frames: both reads
  were consumed by capabilities frames, `ready` was never seen → exit 1,
  and the fallback printed whichever backend sorted first — the embed one,
  so an operator saw `v2: false, vision: false` presented as the daemon's
  own answer. A fixed count cannot be right when the frame count is a
  function of backend count. Latent on a single-backend config, which is
  why no earlier validation pass caught it; reproduced identically on
  Windows named pipes and Linux UDS, and against the v0.6.1 CLI, so it is
  not a v0.7.0 regression.

  `status` now reads until the lifecycle frame arrives (bounded by
  `STATUS_MAX_FRAMES = 64` and the existing 500 ms timeout) and prints
  **every** frame — one `capabilities` line per backend, then the
  lifecycle line. Printing all of them rather than selecting "the"
  generation backend is deliberate: ADR 0007 lets a config register
  several generation backends, so picking one would only relocate the
  arbitrary choice. `doctor` already reports one line per backend for the
  same reason. Frames keep the daemon's order, so `status | tail -1` is a
  stable idiom for the lifecycle line alone. Readiness is keyed on a
  `ready` frame actually being seen — `capabilities` describes a backend,
  not the daemon — so a truncated capabilities-only burst is correctly
  *not* ready.

  Verified live against a running two-backend CUDA daemon: three frames,
  both backend names, `ready`, exit 0. Nine new unit tests cover the
  three-frame burst, the single-backend case, capabilities-only,
  a pre-`ready` snapshot, stopping at the snapshot rather than eating live
  events, the frame bound, a stalled peer, an immediately closed socket,
  and frame ordering. Testing the read loop needed a
  `#[doc(hidden)] AdminClient::wrap_for_test` — the same escape hatch
  `ClientV2`, `EmbedClient` and `RerankClient` already expose;
  `AdminClient` was the only one without it, which is why nothing
  exercised this loop before.

  One of those tests needs `#[tokio::test(start_paused = true)]`, which
  required adding tokio's `test-util` as an explicit `inferdctl`
  dev-dependency. It compiled locally without it only because feature
  unification with the sibling crates' dev-deps supplied `test-util` by
  accident; the airgapped `--no-default-features` CI job does not unify it
  and failed to build the test binary. `inferd-client`, `inferd-daemon` and
  `inferd-http` already declared the same dev-dep — `inferdctl` was the
  omission. Any workspace crate whose tests use a tokio test feature must
  declare it rather than rely on a sibling.

- **systemd unit could not start on a fresh install: `ExecStartPre` ran
  inside the mount namespace it was meant to make constructible**
  (`packaging/systemd/inferd.service`, issue #59). `ProtectHome=read-only`
  plus `ReadWritePaths=%h/.local/share/models %h/.inferd` means systemd
  resolves both paths during mount-namespace setup and aborts with
  `226/NAMESPACE` if either is missing — which on a machine that has never
  run inferd is both of them. The unit already carried an
  `ExecStartPre=/usr/bin/mkdir -p` for exactly this reason, but namespace
  setup runs per *command*, ahead of every `Exec*` line including
  `ExecStartPre`, so that mkdir was sandboxed by the very carve-out it
  existed to satisfy and died before creating anything. The journal blames
  `(mkdir)`, which reads like a missing binary rather than a sandbox
  failure, and `StartLimitBurst=3` then converted the whole thing into
  `Start request repeated too quickly` within ~9 seconds — pointing away
  from the cause on all three counts. Fixed by prefixing the command with
  `+`, which runs it outside the sandbox. `+` grants no privilege here:
  this is a `systemctl --user` unit, so the mkdir runs as the same
  unprivileged user either way; the prefix only opts that one command out
  of the namespace restrictions. The unit's comment asserted the opposite
  ordering ("systemd resolves those paths … *before* ExecStartPre runs")
  and is corrected in the same change.

  Verified on real systemd 255 against the shipped
  `inferd-v0.7.0-x86_64-unknown-linux-gnu` binaries from a genuinely
  fresh state, with the pre-fix unit as a control: unfixed → three
  `226/NAMESPACE` aborts and neither directory created; fixed → zero
  aborts, both directories created, unit `active`, first-boot
  `config.json` written, all three sockets bound at the modes invariant #6
  requires (`admin.sock` `0600`, `inferd.sock` / `infer.embed.sock`
  `0660`), and a restart with the directories already present still clean
  (`mkdir -p` is idempotent). Two probes confirm the sandbox was not
  merely widened: `$HOME` outside the carve-outs is still read-only to the
  unit, and `%h/.inferd` is writable from inside it.

  Why no gate caught this: the `systemd-unit` CI job installs and starts
  the real unit on a fresh runner, but never asserted that the two
  `ReadWritePaths=` targets were *absent* first, so it could not
  distinguish a fresh install from a warm one and stayed green. It now
  deletes both paths and asserts they are gone before starting, fails on
  `226/NAMESPACE` by name (a packaging bug in the unit, distinct from a
  daemon crash), and asserts afterwards that `ExecStartPre` created both
  and that the first-boot config landed. Every prior Linux validation pass
  ran on a box that already had `~/.inferd`. Found by the issue #56
  airgapped-archive gate.

- **Installers told the operator to run `inferdctl`, then never installed
  it** (`packaging/windows/install.ps1`,
  `packaging/windows/uninstall.ps1`,
  `packaging/launchd/install-launchagent.sh`, `README.md`, issue #58). Both
  scripted installers close by printing `inferdctl status` / `inferdctl
  watch` / `inferdctl import` — and on an airgapped build (ADR 0028)
  `import` is the *only* way a model reaches the store, so the CLI sits on
  the critical path of the runbook they print. Neither script put it
  anywhere the operator could run it from. The two platforms needed
  different fixes because their installers have different shapes:

  - **Windows** stages into a fixed `%LOCALAPPDATA%\inferd`, so the CLI is
    staged there too, next to the daemon it talks to, and that directory
    is appended to the **user** `PATH`. The append reads
    `[Environment]::GetEnvironmentVariable("PATH", "User")` specifically,
    never `$env:PATH` — the process variable is machine `PATH` plus user
    `PATH` concatenated, so writing it back to User scope would copy every
    system entry into the user's own `PATH` and permanently double it. It is
    idempotent (trailing-slash-normalised membership test; `-contains` is
    already case-insensitive) and **declines to write at all** in the two
    cases where it cannot do so losslessly, warning and falling back to the
    fully-qualified path in the closing message — the install is complete
    either way, and the only thing lost is the convenience of a bare
    `inferdctl`:

    - `PATH` longer than 2000 characters, because `HKCU\Environment`'s
      `PATH` is practically capped near 2048 and a longer write can
      truncate and cost the user unrelated entries.
    - `PATH` stored as **`REG_EXPAND_SZ`**, i.e. holding literal
      `%USERPROFILE%\bin`-style tokens. The .NET accessors cannot
      round-trip that: the getter returns the *expanded* string and the
      setter writes `REG_SZ`, so an append would silently bake today's
      expansion in and downgrade the value kind, permanently destroying the
      user's indirection. Rewriting it faithfully means writing the
      registry directly plus a `WM_SETTINGCHANGE` broadcast that
      `SetEnvironmentVariable` performs for free — more machinery, and more
      ways to damage `PATH`, than the convenience justifies.

    `uninstall.ps1 -Purge` removes the entry again rather than stranding a
    dead path, and declines symmetrically on `REG_EXPAND_SZ` (if the kind
    is `ExpandString`, the installer never added the entry, so there is
    nothing to remove and flattening the user's `%VAR%` references would be
    pure damage).
  - **macOS** needs no staging and gets none: `install-launchagent.sh`
    deliberately never relocates the daemon — the plist points launchd at
    wherever the archive was extracted, so that directory already *is* the
    install directory and already contains `inferdctl`. Copying it
    elsewhere would create a second CLI copy free to drift from its
    daemon. The fix there is a resolvable invocation: prefer a bare
    `inferdctl` only when the one already on `PATH` is *this* one, else
    print the absolute path.

  Both scripts now also degrade honestly. If no `inferdctl` is found next
  to the daemon, they say so where they previously printed a command that
  resolved to nothing, and the CLI is reported alongside the binary in the
  closing summary. `inferd-http` is deliberately *not* staged: it is a
  separate, user-launched consumer (ADR 0020 / ADR 0014) and nothing these
  installers print asks the operator to run it.

  Verified on Windows against a fake archive with every path redirected to
  scratch: CLI staged, `PATH` grew by exactly the directory length, re-run
  left `PATH` byte-identical with one occurrence of the entry, a
  CLI-less archive produced the degraded message and staged nothing, a
  `REG_EXPAND_SZ` `PATH` came through both install and uninstall with its
  value kind and `%USERPROFILE%` token untouched (warning emitted, absolute
  path printed), and `-Purge` restored the `PATH` byte-for-byte. The macOS
  change had no hardware coverage when it landed (`bash -n` only); the #60
  macOS leg of #56 has since **confirmed both branches on real hardware**
  — see the Validation entry above. The README's Linux steps already
  copied the CLI onto `PATH`; its macOS and Windows steps claimed a bare
  `inferdctl` would resolve from the archive and are corrected to match
  what each installer actually leaves behind.

## [0.7.0] - 2026-08-05

Minor, not patch: **a fourth wire surface**. Rerank on
`infer.rerank.sock` ([ADR 0027](docs/adr/0027-reranking-on-a-fourth-socket.md))
is purely additive — `wire_version` is unmoved, v0.6.x clients
interoperate unchanged, and a daemon whose model has no classification
head binds no rerank socket — but a shipped surface is a frozen surface,
so this tag is what fixes its shape. Also ships a **second archive per
platform** ([ADR 0028](docs/adr/0028-airgapped-build-profile.md)): the
same commit built `--no-default-features`, with no HTTPS client linked
at all, for hosts that must load models via `inferdctl import`. Ten
archives now, across five platforms.

### Validation

- **macOS arm64 Metal — airgapped build profile (ADR 0028), all 7 steps
  green, no macOS-specific defects (2026-08-05):** completes issue #55's
  cross-platform matrix (Windows CUDA + Linux/WSL CPU already green).
  Built from `main` (`cargo build --release --no-default-features
  --features inferd-daemon/dl-backends`, no airgapped tarball exists
  yet). Step 1: all 14 `no-network-deps` assertions clean +
  anti-vacuity control confirms the check is meaningful. Step 2: both
  binaries self-report `build profile: airgapped`. Step 3: a
  `source_url`-bearing config fails loudly (`fetch failed …
  airgapped build (no model-fetch feature); import it with
  'inferdctl import'`), Metal accelerator probe unaffected. Step 4:
  `inferdctl import` lands both `gemma-4-e4b` and `embeddinggemma-300m`
  into a fresh store with correct CAS layout/manifest, SHA verified,
  idempotent re-import, deliberate mismatch rejected (exit 2, no
  manifest written). Steps 5/6: real generate (`"AIRGAPPED"`,
  `backend=llamacpp`) and real embed (256-dim, L2=1.0) from a store
  only `import` filled; socket modes match invariant #6. Step 7: the
  real `install-launchagent.sh` correctly detected the airgapped
  profile and pointed at `import`, not `watch`. See
  `docs/v0.6-validation.md`, issue #55.

### Added

- **Cross-encoder rerank on a fourth socket** ([ADR 0027](docs/adr/0027-reranking-on-a-fourth-socket.md),
  task #171). A new inference surface — `infer.rerank.sock` /
  `\\.\pipe\inferd-infer-rerank`, NDJSON framed like embed — that scores a
  query against each candidate document *jointly*: one model forward pass
  per document, nothing precomputable. It belongs downstream of retrieval
  (`embed → top-50 → rerank → top-5 → generate`), which is where the
  precision gain over vector similarity alone comes from. Additive: no
  existing surface changed, `wire_version` unmoved, and a daemon whose
  model has no classification head binds no rerank socket, so existing
  deployments see nothing new.
  - **A separate surface rather than a flag on embed**, because a
    cross-encoder is a different computation, not a different pooling
    option. `pooling_type` is fixed at `llama_context` creation, so
    `LLAMA_POOLING_TYPE_RANK` — which attaches the model's classification
    head to the graph — genuinely needs its own context. Requests carry
    `{query, documents[], top_n?}` and get back `{index, score}` pairs
    **already sorted descending and truncated to `top_n`**; the daemon
    owns the ordering because score scales are model-specific and
    re-deriving it per consumer invites drift.
  - **Scores are raw, never normalised.** They are ordinal *within one
    response only* — never comparable across models, requests, or against
    a fixed threshold, and negative values are ordinary (most
    cross-encoders emit logits). Squashing them into a synthetic `0..1`
    would make incomparable numbers look comparable, which is the more
    expensive failure.
  - **Bounded at parse: `MAX_RERANK_DOCUMENTS` (256) and
    `MAX_RERANK_TOTAL_BYTES` (8 MiB).** Rerank is the one surface whose
    cost is `O(documents)` forward passes, so the 64 MiB frame cap bounds
    the wrong thing: one cheap in-cap frame of short documents describes
    on the order of half a million full model evaluations, all holding the
    shared admission permit. Same amplification class as THREAT_MODEL F-1.
    Both constants are re-exported from `inferd-client` so callers can
    pre-trim rather than discover the cap on a rejected request.
  - **Preconditions fail the *load*, not the request.** A model with no
    BOS token, or with no way at all to mark the query/document boundary
    (no EOS, no SEP, no `rerank` chat template), is rejected at
    construction — so per invariant #5 the socket is never bound. There is
    no runtime signal for this: the classification head returns a float
    either way, so a wrong model produces *scores*, just meaningless ones.
  - **Discoverable before dialling.** The admin `capabilities` frame
    gained a `rerank` flag alongside `embed`, surfaced by `inferdctl
    doctor`, `AdminEvent.rerank` (Rust) and `AdminEvent.SupportsRerank()`
    (Go), plus `DefaultInferRerankAddr()` for the Go socket path. It is
    omitted when false, so a v0.6.1 subscriber sees a byte-identical
    frame, and the two flags are independent — a bi-encoder reports
    `embed: true, rerank: false`.
  - `RerankClient` in `inferd-client`; `Backend::rerank` with a default
    `Unsupported` impl so existing adapters are unaffected;
    `capabilities().rerank`; `Router::dispatch_rerank`; config
    (`rerank`, `rerank_n_ctx`, default off — a rerank context is a second
    context plus KV cache, and a deployment doing no retrieval must not
    pay for it). `rerank_unsupported` is the fail-safe error code.
  - Tested end-to-end at Tier 2 (12 tests over a real UDS **and** a real
    named pipe — the first daemon integration test to cover both rather
    than being Unix-gated) and Tier 3 against a real `bge-reranker-v2-m3`
    (8 tests, `INFERD_TEST_RERANK_MODEL_PATH`). Tier 3 is load-bearing
    here: the mock scores by word overlap, so a rerank path that reads the
    wrong buffer entirely still passes every mock test with plausible
    numbers in a plausible order. Two assertions only a real model can
    make — the same document must score *differently* under two unrelated
    queries (a bi-encoder-shaped bug scores it identically), and identical
    pairs must score *identically* across a batch (drift means the KV
    cache isn't cleared between passes).
- **Backends advertise the audio sample rate they require.**
  `BackendCapabilities` gained `audio_sample_rate: Option<u32>`, published
  on the admin `capabilities` frame as `audio_sample_rate` (omitted when
  the backend takes no audio). The llamacpp adapter reads it from the
  loaded mmproj once at init. Backwards-additive on the admin wire; the
  generation wire is unchanged.
- **`inferd-http` accepts OpenAI `input_audio` — speech in over the
  OpenAI-compat bridge** ([ADR 0025](docs/adr/0025-bridge-decodes-and-resamples-audio.md),
  task #200). A `user` message part
  `{"type":"input_audio","input_audio":{"data":"<base64>","format":"wav"}}`
  is decoded (wav/mp3, via `symphonia`), downmixed to mono, **resampled**
  (via `rubato`) to the rate the daemon requires, and sent as an inferd
  `Attachment::Audio`. Resampling in the bridge is what makes the feature
  usable at all: real clients record at 44.1/48 kHz, the daemon accepts one
  rate and rejects the rest (it never resamples — ADR 0016), and no OpenAI
  SDK converts audio. The daemon is unchanged; no wire change.
  - **The target rate is read from the daemon, per audio request**, off the
    admin socket (`--admin-addr-override` added for a non-default path) —
    never hardcoded. A cached rate would survive a daemon restart onto a
    different mmproj and then produce confidently-wrong transcription,
    which is the exact failure the rate contract exists to prevent.
    Text/image-only requests never pay for the probe. No advertised rate →
    400, not a guess.
  - Bounded three ways against decompression bombs: encoded payload (8
    MiB), decoded sample count (checked *during* the decode loop, so a
    small mp3 claiming hours fails partway), and predicted resampled
    payload size (checked before the work). Max 4 clips per request, `user`
    messages only. No SSRF surface — OpenAI defines no audio URL form.
  - The per-request decoded-attachment byte budget is now **shared across
    modalities** (`MAX_TOTAL_DECODED_ATTACHMENT_BYTES`, renamed from the
    image-only constant) to mirror the daemon's aggregate
    `MAX_ATTACHMENT_BYTES_PER_REQUEST`; two independent budgets would let
    the bridge build a request the daemon refuses.
  - `inferd-client` now re-exports `AdminError`, which was already the
    error type of a public API but had never been exported.
  - **Licence note:** `symphonia` is MPL-2.0, the first non-permissive
    dependency in the tree. It is confined to the `inferd-http` **binary**;
    the daemon, the engine, and both crates.io-published libraries
    (`inferd-proto`, `inferd-client`) do not link it. inferd stays MIT
    (ADR 0004); rationale and scope in ADR 0025.
- **`deny.toml` + a PR-blocking `licenses` CI job.** Until now "inferd is
  MIT and so is everything under it" was true by accident and verified by
  nothing — `cargo deny check` was in the documented pre-commit gate with
  no config file to read and no CI job to run it. The MPL-2.0 dependency
  above made the claim non-trivial, so it is now machine-checked: a licence
  allow-list, and a `[[bans.deny]]` entry asserting `symphonia` is
  reachable **only** from `inferd-http` (verified load-bearing — pointing
  the wrapper elsewhere fails the check). This job *does* block PRs, unlike
  `cargo audit`: a licence check reads `Cargo.lock` and cannot start
  failing because someone published an advisory overnight.
- **Both client libraries can now read the required audio sample rate.**
  The daemon advertised `audio_sample_rate` but neither
  `inferd_client::AdminEvent` nor the Go `AdminEvent` carried the field, so
  no supported consumer could see the contract it is required to honour.
  Added to both, plus Go's `SupportsAudio()` / `RequiredAudioSampleRate()`
  accessors (mirroring `SupportsVision()`) and an `AudioAttachment(id,
  sampleRate, pcmF32LE)` constructor — the Go client had one for images but
  not audio, so callers hand-built the struct and could omit `SampleRate`
  entirely. Found by running the audio path live rather than by review.
- **An airgapped build, and a second archive per platform to ship it**
  ([ADR 0028](docs/adr/0028-airgapped-build-profile.md), task #145). Every
  release now publishes `inferd-airgapped-vX.Y.Z-<target>` alongside
  `inferd-vX.Y.Z-<target>` — the same commit built
  `--no-default-features`, in which no HTTPS client is linked at all:
  `ureq`, `rustls`, `ring`, `webpki-roots` and the native certificate store
  are absent from the binary rather than present-but-unreachable. Models
  arrive instead via a new `inferdctl import`. Nothing changes for existing
  users: the networked archive is the default build, byte-for-byte the same
  configuration as before, and no wire surface moved.
  - **The feature had to be `model-fetch`, default-on, rather than an
    `airgapped` flag.** Cargo features are purely additive, so an additive
    `airgapped` feature could only `#[cfg]`-out the *call sites* while
    leaving the entire TLS stack linked into the artifact — the most
    dangerous kind of wrong, because it would appear to work and pass any
    functional test. Polarity is inverted so that removing the capability
    removes the dependency.
  - **The deliverable is the `cargo tree` assertion, not the `#[cfg]`
    attributes.** A third party can reproduce the no-egress property
    against a tag without reading inferd's source; the CI `no-network-deps`
    job runs exactly that on every PR, over the daemon and `inferdctl`,
    with a control check asserting the *default* tree still matches so the
    detector cannot pass vacuously. `hyper` is deliberately not banned by
    name: it is a protocol library with independently-gated halves, and
    `inferd-http` (an inbound localhost listener, ADR 0020) links the
    server half — a separate step asserts the `client` feature is absent
    instead, which is the property that actually matters.
  - **`inferdctl import --name <NAME> [--expect-sha256 <HEX>] <PATH>`**
    hashes while copying into the CAS store through the same
    partial-then-rename producer flow `pull` uses, re-reads the landed
    bytes, and writes the manifest last. `--expect-sha256` is compared in
    constant time before anything is written. It ships in **both**
    archives — a subcommand shipped only in the hardened build is a
    subcommand nobody tests, and importing a hand-downloaded GGUF is
    useful on a networked host too.
  - **`source_url` may now be empty**, meaning "resolve from the model
    store only". This was a hard blocker rather than a nicety: config
    validation required an `https://` prefix, so an airgapped operator
    could not write a valid `config.json` at all, even though the resolver
    already handled the manifest-only path. `sha256` stays required and is
    still constant-time verified on every boot.
  - **Both binaries report which build they are** — `--version` prints
    `build profile: networked|airgapped` and the daemon logs `build=…` on
    its first activity-log line. The string is a `concat!` const selected
    by the same `cfg` that decides whether the HTTPS stack is linked, so
    the two cannot disagree. The installer scripts (shared by both
    archives) ask the binary rather than assuming, so none of them can
    keep promising a model pull that cannot happen.
  - Release staging was extracted to `packaging/stage-release.sh` and is
    now called twice per platform: two archives assembled by two copies of
    an inline script is how their *contents* start to diverge, and "the
    two artifacts are one code path" is the premise of the ADR. Asset
    completeness is counted per variant, not as a bare total.
  - Not gated, deliberately: the `fetch` module's local half — manifest →
    CAS resolution, the per-name writer lock, the constant-time re-hash,
    quarantine on mismatch. An airgapped deployment needs all of it.
  - New runbook: [`docs/airgapped.md`](docs/airgapped.md) (import →
    config → run), shipped inside both archives because an operator on a
    disconnected machine cannot open the repo to read it.
  - **Model names are now validated where they become paths.** `import`
    took a name from `--name` and interpolated it into the staging, lock,
    and manifest paths; the store's name→path builders are the only
    non-content-addressed part of the ADR 0011 layout, so that is the
    validation boundary. `store::validate_model_name` enforces a
    conservative grammar (ASCII alphanumeric plus `.`, `-`, `_`; no
    leading `.`, no `..`, ≤ 128 bytes — which also excludes path
    separators and NTFS alternate data streams), plus an explicit reject
    of Windows reserved device names, which the character set does *not*
    cover: `CON` is plain ASCII, and Win32 resolves a reserved base name
    to the device regardless of extension or directory, so
    `manifests/CON.json` is not dependably a file. Rejected on every
    platform, because a shared store (ADR 0011) can be written on one OS
    and read on another. `manifest_path`/`lock_path` became fallible so the
    compiler enumerates the call sites rather than leaving each one to
    remember. Two of the three sinks predate `import`; `write_manifest`
    validates before `create_dir_all`, so a rejected name leaves nothing
    behind. Any name previously written by `pull` is unaffected.
  - **Staging writes are handle-bound, not path-bound.** The import
    staging file is opened `create_new` after a non-following
    `symlink_metadata` check (THREAT_MODEL F-2, the same answer
    `endpoint::bind_uds` gives for the socket path), and the post-copy
    re-hash and size read go through *that descriptor* instead of
    re-opening the path — a path re-resolves on every call, so the file
    verified would not necessarily be the file written. The rename onto
    the CAS blob path now treats "the blob is there now" as success:
    the writer lock is per-name but the blob path is per-content, so two
    names importing identical bytes can race, and on Windows renaming
    onto an existing file fails outright.

### Fixed

- **Both Windows legs of the release workflow failed on the airgapped
  build step, which had never once succeeded on Windows.** ADR 0028
  added it as the job's only *multi-line* `cargo build`, using `\`
  continuations — which the Windows default shell (PowerShell) does not
  honour, so it parsed `--bin` as a unary minus (*"Missing expression
  after unary operator '--'"*). The obvious fix, `shell: bash`, then
  fixed the parse and broke the **link** instead: Git Bash puts
  `C:\Program Files\Git\usr\bin` ahead of the MSVC toolchain on `PATH`,
  so rustc invoked coreutils `/usr/bin/link` rather than `link.exe` and
  every build script died with *"extra operand … .rcgu.o"*. The step is
  now two single-line steps with no `shell:` override at all, matching
  the three networked `cargo build` steps that have shipped five
  releases: on the Windows runners a `cargo build` must run under the
  same shell as its neighbours, which means it cannot span lines. Linux
  and macOS default to bash and were never affected, so the first two
  v0.7.0 runs each produced 3 of 5 platforms and published nothing
  (`publish` needs every build leg). Nothing in CI covers this: no PR
  builds the release workflow's Windows path.
- **`inferdctl import` swallowed every config error and silently imported
  into the platform-default store.** The store was resolved with
  `match ConfigFile::load(..) { … Err(_) => default_models_home() }`. The
  catch-all was meant for the documented import-then-configure case (no
  config yet, which is the normal state of a fresh airgapped machine), but
  it also caught unreadable, unparseable, and *invalid* configs — so a
  config the daemon rejects loudly (`invalid config: backends list must
  not be empty …`) made `import` print success while writing somewhere
  the operator had not asked for and the daemon will never open. Worst on
  an airgapped box, where `import` is the only way bytes get in and there
  is no fetch to fail afterwards and reveal the mistake. The fallback now
  matches `ConfigError::NotFound` only; anything else fails with
  `cannot determine the model store: config <path> exists but is unusable
  (fix it, or move it aside to import into the default store)` and writes
  nothing. Found by the ADR 0028 cross-platform validation (issue #55,
  step 4) rather than by a test, so the four arms — absent → default,
  valid → honoured, invalid → error, unparseable → error — are now unit
  tests.
- **The airgapped `cargo tree` assertion was reading rendered output, not
  data — and matched nothing at all in CI.** The `no-network-deps` job
  extracted crate names with a `sed` pattern anchored at `^`, but the
  workflow sets `CARGO_TERM_COLOR=always`, so every line arrived wrapped
  in ANSI SGR escapes and the anchor never sat against the crate name.
  All five `cargo tree` invocations therefore matched zero crates: the
  four airgapped assertions printed `OK` while checking nothing. This is
  precisely the failure mode ADR 0028 was written to prevent — a
  guarantee that reads as proven and was never tested — and the only
  reason it surfaced is the anti-vacuity control, which demands the
  *default* build match the same pattern and went red when it didn't.
  Fixed by asking cargo for machine-readable output once
  (`--prefix none --format '{p}' --color never`) and reading field 1 with
  `cut`, which removes the prefix-stripping regex rather than correcting
  it. The `hyper`-is-server-only step got `--color never` too: its greps
  are unanchored so the escapes did not break them, but that was luck,
  and the next tightened pattern would have inherited the bug. Verified
  under `CARGO_TERM_COLOR=always` locally: all four airgapped trees
  empty, control matching `ring rustls rustls-native-certs
  rustls-pemfile rustls-webpki ureq webpki-roots`, and
  `inferd-engine --features openai` still caught as TLS-linking. The
  runbook command in `docs/airgapped.md` carries the flag and the reason
  as well, since an operator with that variable exported would have
  reproduced the same false pass by hand.
- **Tier 3 has not compiled since 2026-06-30 and nobody noticed.** The
  real-model integration tests (`crates/inferd-engine/tests/llamacpp.rs`)
  build a `ResolvedV2` literal directly, so the `thinking` field added by
  the Gemma 4 thinking-activation work (#173) broke the file — five weeks
  and two releases ago, spanning v0.6.0 GA and v0.6.1. It went unseen
  because Tier 3 is invoked by neither `cargo test --all` nor any
  documented clippy variant, and it *skips silently* without
  `INFERD_TEST_MODEL_PATH`, so a green local gate and a green CI run both
  looked identical to a passing Tier 3. This is the same feature-gated
  struct-literal trap `CLAUDE.md` already warns about, in the one tier the
  same file calls "mandatory, not optional" for prefill/templating/FFI
  changes — and it was found only by running Tier 3 to verify unrelated
  work. Fixed by adding the missing field.
- **Use-after-free of the `llama_model` at teardown** (issue #202). Every
  C handle derived from a loaded model reads back through it when it is
  destroyed: `llama_context` stores `const llama_model & model`
  (`llama-context.h:276`) and its destructor dereferences
  `model.hparams.no_alloc` plus the ggml scheduler buffers hanging off it
  (`llama-context.cpp:420`), and `mtmd_context` caches
  `llama_model_get_vocab(text_model)` (`tools/mtmd/mtmd.cpp:311`). The
  adapter's `State` declared `model` as its **first** field, and Rust drops
  struct fields in declaration order — so `llama_model_free` ran first and
  the generation context, the embed context, and the mtmd context were then
  each torn down against freed memory. On Windows this surfaced as
  `0xC0000005` (STATUS_ACCESS_VIOLATION) at process exit; it was
  deterministic, not flaky (6/6 runs of the Tier 3 embed binary crashed
  before the fix, 6/6 exit `0` after).
  - Because backends are held in an `Arc` for the life of the daemon
    process, the only teardown was at exit, so the practical impact was a
    crashing shutdown rather than corruption during a request. It still
    meant the daemon could never report a clean exit status, and it made
    Tier 3 impossible to wire into any gate.
  - What made this durable is worth recording: the struct carried comments
    asserting the drop order was "mtmd → ctx → model" and that `Mtmd`
    "borrows the parent `ModelHandle` … the Rust borrow makes that
    explicit". Neither was true — no borrow was ever taken, the constructor
    took a raw `*const llama_model` — and the comments read as though the
    hazard had been handled. So the fix makes the lifetime structural
    instead of documentary: `ModelHandle` is now an `Arc` and every derived
    handle holds a clone, which keeps teardown correct no matter how the
    fields are later reordered. `Mtmd::new` consequently drops its `unsafe`
    marker, since the guarantee it used to demand of callers is now
    discharged by its own signature.
- **A Tier 3 test asserted against an API that no longer existed.**
  `rejects_invalid_messages` expected `generate_v2` to return
  `InvalidRequest` for an empty `messages[]`, which was true when the
  renderer returned `Option` and yielded `None`; the `ChatRenderer` trait
  returns `Result` and no implementor treats "no messages" as an error, so
  the assertion had been failing — invisibly, because the teardown crash
  above killed the binary before it printed a summary. Empty `messages[]`
  is rejected at the proto boundary (`RequestV2::resolve`), which is the
  only place it *can* be rejected now that `ResolvedV2` is constructible
  only via `resolve()` outside tests, so the test now asserts against that
  gate rather than re-adding a redundant engine-layer check. A second test
  covers a rejection the engine does own (`max_tokens: 0`).
  - With both fixed, all four Tier 3 binaries pass **and exit `0`** on
    Windows for the first time: `llamacpp` (4), `embed_llamacpp` (9),
    `llamacpp_multimodal` (2), `grammar_llamacpp` (3).
- **The bridge no longer misreports an out-of-date daemon as an
  audio-incapable one.** A daemon older than the `audio_sample_rate` field
  advertises `audio: true` with no rate; the bridge returned *"the daemon's
  active backend does not accept audio input"*, which is false — it sends
  the operator auditing their model instead of their daemon version. The
  capabilities probe now distinguishes "no audio backend" from "audio
  backend, no advertised rate" and tells the latter to upgrade the daemon.
  Covered by unit tests that fold **verbatim live caps frames** (with rate,
  without rate, embed-only) so the two cannot silently collapse again.
  Found by pointing the new bridge at the v0.6.1 GA daemon.

- **An audio attachment at the wrong sample rate is now rejected instead
  of silently mis-decoded.** `Attachment::Audio::sample_rate` was declared
  on the wire and read by nothing, and `mtmd_get_audio_sample_rate` was
  wrapped with zero call sites. Because libmtmd's audio entry point takes
  no rate argument, PCM at the wrong rate is not a detectable error — 44.1
  kHz fed to a 16 kHz encoder is time-scaled ≈2.75× and the model returns a
  plausible wrong answer. A mismatch now fails with `invalid_request`
  naming both the declared and required rates. The daemon still does not
  resample (ADR 0016 — the consumer decodes, so it owns rate conversion);
  consumers read the advertised rate and convert before sending.
  **Now proven live** (task #199, `docs/v0.6-validation.md`): the shipped
  Gemma 4 E4B mmproj advertises 16000 Hz; 7 s of 16 kHz speech transcribed
  **5/5 ground-truth phrases verbatim**; the same PCM declared at 44100 Hz
  rejected naming both rates. Audio had never run against a real model on
  any platform before this.

### Changed

- **The landing site documents rerank.** The three surface cards
  (generate / embed / admin) never mentioned it, so the wire surface
  shipping in this release was invisible on the site. A fourth card
  now covers it, and `.grid-three` became `.grid-surfaces` with an
  `auto-fit` track so the next surface doesn't need a CSS edit. The
  architecture diagram's socket list and the README's "ships today"
  and `inferdctl` subcommand lines were also stale.
- **The README no longer implies v0.7.0 is install=work validated.**
  It claimed validation on three platforms while pointing at
  `docs/v0.6-validation.md` — true of the v0.6.1 tarballs, not of this
  tag, whose archives were still building when it was written. It now
  says which tag was validated and points at issue #56 for this one.
- **The chat renderer is now a registry keyed to model family, and a model
  whose prompt format inferd does not know fails to load**
  ([ADR 0026](docs/adr/0026-chat-renderer-registry-per-model-family.md),
  task #201). Until now there was exactly one renderer, Gemma 4's, applied
  unconditionally to whatever GGUF was configured. Point the daemon at a
  non-Gemma model and it would wrap that model's turns in `<|turn>` /
  `<turn|>` markers the tokeniser has never seen, and generate fluently
  against a prompt the model cannot parse — no error, no warning, just a
  wrong answer with a plausible shape. What made it worth fixing now rather
  than at the next model bump: the architecture string alone cannot be the
  key. `ibm-granite/granite-docling-258M`'s text tower declares
  `model_type: "llama"`, so its GGUF `general.architecture` is
  byte-identical to Llama-3-Instruct's, whose grammar is unrelated.
  - `ChatRenderer` is now a trait with two implementors (`Gemma4Renderer`,
    unchanged in behaviour and still byte-exact against
    `docs/text.function.calling.with.gemma.4.md`; `GraniteRenderer`,
    transcribed from `granite-docling-258M/chat_template.jinja` and
    corroborated against llama.cpp's own `LLM_CHAT_TEMPLATE_GRANITE_3_X`).
    The second implementor exists to prove the seam is real rather than a
    one-renderer abstraction.
  - The family is resolved **once at model load**, not per request: an
    explicit `chat_template` field on the backend entry wins (also
    `--chat-template` / `INFERD_CHAT_TEMPLATE` for the `--model-path` dev
    path); otherwise it is detected from the loaded GGUF's own metadata,
    pairing `general.architecture` with a fingerprint of
    `tokenizer.chat_template`. No new FFI — the existing
    `llama_model_meta_val_str` reader is reused.
  - **The behaviour change:** a model that carries a chat template matching
    no known family now **fails the load** with a message naming the
    architecture, a bounded fingerprint of the template, and the families
    inferd knows. It does not fall back to Gemma. This is safe to do
    loudly because socket binding already happens strictly after backend
    readiness (invariant #5), so the failure is a daemon that refuses to
    start rather than one serving wrong answers. A model with **no** chat
    template at all (every embedding model, including the
    `embeddinggemma-300m` in the first-boot default config) is not an
    error: it resolves to no renderer, and `capabilities().v2` is now
    `false` for such a backend, so the generation socket simply is not
    bound for it.
  - Capabilities are now family-derived rather than constant: `tools` and
    `thinking` report what the resolved renderer actually implements.
    Granite's template carries no tool or thinking grammar, so a request
    to a Granite model carrying `tools[]`, a `tool_use`/`tool_result`
    block, or `thinking: true` is **rejected** rather than rendered with
    the tools silently dropped — same reasoning as the audio rate contract
    above: a request the model was never told about produces a fluent
    wrong answer, which is worse than an error.
  - No wire change. `wire_version` is unmoved and the media path needed no
    per-family work — `<__media__>` is mtmd's marker, not Gemma's, and
    mtmd substitutes the per-architecture fences itself.
- **The README and the landing site now say inferd does audio.** Both
  described the daemon as multimodal but enumerated only vision — the
  reference model has shipped an audio projector in the same mmproj the
  installer already pulls, and the bridge takes `input_audio`, so the copy
  understated what a fresh install does. Both surfaces now name vision
  *and* audio, and both explain the rate contract (the backend advertises
  one rate, a mismatch is rejected rather than resampled, because the
  encoder cannot detect a wrong rate — it returns a fluent wrong answer)
  along with the bridge as the way to avoid converting audio yourself.
  Also fixes a `.callout-block` paragraph-spacing gap the site had never
  hit, since every callout until this one was a single paragraph.
- **`docs/test-strategy.md` now describes the multimodal and bridge test
  coverage it had been silent about.** Tier 1 never mentioned
  `inferd-http` even though `cargo test --all` has always run its 51 tests
  (translation, audio decode/downmix/resample, the `MAX_PCM_BYTES` budget,
  the admin rate probe), and Tier 3's "exercises" list named grammar,
  cancellation, queue and embed but neither vision nor audio — the two
  paths that *only* exist at Tier 3. It now also states plainly that the
  committed multimodal test asserts `input_tokens` rose and never checks
  content, so a regression that garbles an image or time-scales a clip
  would pass CI, and points at the manual per-platform runs in
  `docs/v0.6-validation.md` as the evidence of record until those
  assertions are tightened. Documentation only; no test behaviour changed.
- **Install snippets in `README.md` pointed at `v0.5.0` tarballs.** The
  status badge said v0.6.1 while all three snippets (Linux, macOS,
  Windows) named `inferd-v0.5.0-*` archives, so a reader copy-pasting the
  documented install hit a 404 — and because the README ships *inside*
  every release tarball, the stale copy was in the artifacts too. Root
  cause was that `docs/RELEASING.md` §2 never mentioned the user-facing
  version strings that aren't derived from `Cargo.toml`; it now lists them
  (README status line + three snippets, `site/index.html` masthead /
  download button / snippets / colophon) with a verify grep against the
  previous version, so the drift can't repeat silently. The README's
  Layout tree was also two crates out of date (`inferd-openai-wire`,
  `inferd-http`) and pointed at the superseded `protocol-v1.md`.
- **The published crate READMEs now document the attachment contract.**
  `inferd-proto` is the canonical cross-language schema reference and
  mentioned images once and audio never; it now specifies both variants'
  descriptor fields and BLOB payload layout, that audio is **explicitly
  little-endian** float32 rather than native-endian (which happens to work
  on every platform inferd ships and would be a latent bug on a big-endian
  consumer), and the rate contract. `inferd-client` gained a worked
  attachment example plus how to read `audio_sample_rate` off the admin
  socket instead of hardcoding it. `inferd-engine` documents that a
  capability *requirement* left unadvertised is a silent-wrong-output bug,
  and `inferd-daemon`'s invariant list gained the F-1 attachment bounds,
  the F-17 write timeout, and the no-codec rule — all shipped, none
  written down here. Documentation only.

### Removed

- **`crates-io` job dropped from `release.yml`.** It could not fail
  safely: with no `CARGO_REGISTRY_TOKEN` secret it no-op'd with a
  `::notice::` and reported **success**, so a green release run looked
  like `inferd-proto` / `inferd-client` had shipped when they hadn't —
  exactly what happened at v0.6.1. Publishing is now explicitly manual
  (`docs/RELEASING.md` §5, rewritten): two crates, not five, in dependency
  order, from a tree verified identical to the tag, confirmed against the
  registry API. No secret on the repo, and no job that silently does
  nothing. The old job's premise — that corp TLS blocks the crates.io
  upload API, so publishing had to run from CI — is also false; the v0.6.1
  crates were published through it by hand.

### Validation

- **macOS arm64 Metal — audio native wire + bridge validation, no
  defects (2026-08-04):** closes the "not covered on macOS" gap from
  tasks #199/#200. Built from `main` (`cargo build --release
  -p inferd-daemon --features dl-backends,metal -p inferd-http`, no
  tarball yet). Native wire: A (live rate discovery,
  `audio_sample_rate=16000`), B (matching-rate transcription, 5/5
  ground-truth items verbatim, `input_tokens=260`), C (mismatched-rate
  rejection, both rates named) — all reproduce the Windows result.
  Bridge (ADR 0025): 44.1 kHz stereo → decode+downmix+resample →
  verbatim transcription (non-stream + streaming + multi-turn),
  `format` hint ignored in favor of real container probing,
  undecodable-payload / system-message-audio / clip-cap edge cases all
  return the same 400s as Windows, text-only unaffected. Every gate
  from the Windows validation reproduces identically on Metal — a clean
  confirmation pass, no macOS-specific defects. See
  `docs/v0.6-validation.md`.
- **Bridge audio green on Linux (2026-08-04, WSL2 Ubuntu 26.04, kernel
  6.18.33.2, built from `main`, accelerator `cpu`).** Audio had only ever
  run on Windows, and everything platform-sensitive in the path lives in
  the decode half (`symphonia` probing, the resampler, LE-f32 byte order),
  so a second OS is the cheap check that 16 kHz mono LE-f32 wasn't a
  Windows-shaped assumption. Same 44.1 kHz **stereo** clip as the Windows
  gate (sha256 `52e0e4b7…`) — wrong on both axes, a rate the daemon rejects
  outright, so a pass can only mean the bridge converted it. Transcript
  **verbatim, 5/5 items** at `prompt_tokens=426`; streaming 37 deltas with
  `[DONE]`; **edge cases 8/8**. And the resampling is proven *load-bearing*
  by bypass: the same PCM sent straight to the daemon over the UDS at
  44100 Hz → `invalid_request` naming both rates. Not a tarball run — the
  v0.6.1 tarball's `inferd-http` predates `input_audio`, so no tarball
  could pass this gate; that moves to the next tag, as does any
  non-Windows **GPU** box (this run was CPU). Details in
  `docs/v0.6-validation.md`.
  - **Finding: the daemon emits two `capabilities` frames and the first
    says `audio: false`** (embed-only backend), the second
    `audio: true, audio_sample_rate: 16000`. The bridge's non-latching
    `AudioSupport::fold` is therefore load-bearing on real hardware, not a
    defensive nicety — a fold that latched on the first frame would have
    disabled audio outright. Related: `inferdctl status` shows one
    backend's capabilities even when two are registered, so `status` alone
    cannot confirm audio support.
- **Audio is therefore validated on all three desktop OSes** — Windows
  (CUDA), macOS (Metal), Linux (CPU) — validated independently, with
  `audio_sample_rate=16000` reported identically by all three and every
  gate reproducing. The rate contract holds across the axes it could
  plausibly have varied on. What is still open is narrow and stated:
  audio from a release **tarball** (no tag ships it yet) and audio on a
  non-Windows host with a **discrete GPU**.
- **Windows x86_64 CUDA — v0.6.1 install=work green (2026-08-02):**
  zip sha256 `b3d57ddf…3ed938` verified against the release manifest →
  the bundled `install.ps1` exercised the **upgrade-over-running** path
  (stopped the live 0.6.0-rc.3 daemon before staging), flattened 16
  `backends\*` DLLs → all three binaries report `0.6.1`, `ready` in 1 s,
  `accelerator=cuda`, `device: CUDA0 vram=15.9 GiB`. Native gates 8/8 over
  `\\.\pipe\inferd` and bridge gates 6/6 via `openai` SDK 2.45.0 against
  the zip's own `inferd-http.exe`. Includes the new **G4b** (33
  attachments → `invalid_request`, F-1) and **F-17** confirmed in the
  activity NDJSON (`write_timeout=60s`).
- **Linux x86_64 CUDA (WSL) — v0.6.1 install=work green (2026-08-02):**
  tarball sha256 `5fe0b9fd…` verified → 26 `backends/*` libs flattened,
  booted with the exact `packaging/systemd/inferd.service` ExecStart argv
  → `ready` in 5–6 s on UDS, socket modes `0600`/`0660` as specified,
  `accelerator=cuda`. Native gates 8/8, bridge gates 6/6, F-17 logged.
  **G3 grammar now passes natively on Linux, closing the gap the 0.6.0
  line recorded for this platform.**
- **macOS arm64 Metal — v0.6.1 full G1–G6 tarball pass green
  (2026-08-02):** SHA256 verified. `inferd-daemon`/`inferdctl`/
  `inferd-http` all `0.6.1`. Installed via `install-launchagent.sh` →
  `status=ready`, `wire_version=1`, `accelerator=metal`,
  `device=MTL0 vram=11.8 GiB`. Activity log confirms F-17
  (`response write timeout configured write_timeout=60s`). 16 GiB box
  (< 20 GiB auto-select threshold) → correctly picked E4B. G1 text
  (Paris). G2 thinking (reasoning separated, answer 42, no channel
  leak — confirms the double-`llama_sampler_accept` fix didn't disturb
  the non-grammar path). G3 grammar native (schema-conforming JSON;
  malformed schema fails closed — confirms the grammar-loop fixes
  didn't regress correctness). G4 vision native (exact OCR
  transcription, `input_tokens=73` — confirms F-1's new per-request
  attachment bounds don't false-positive-reject a normal single-image
  request). G5 bridge (chat non-stream/stream, embeddings float+base64).
  G6 bridge vision+grammar (exact OCR + schema-conforming JSON). All
  green, no regressions. See `docs/v0.6-validation.md`, issue #53.
- **Windows arm64 un-parked:** the `aarch64-pc-windows-msvc` leg built and
  published in the 0.6.1 release run (task #185 fixed by
  `GGML_OPENMP=OFF`), so the release matrix is five platforms again. No WoA
  hardware here, so that archive is built + signed only, not install=work.
- **Not covered, stated explicitly:** Windows/Linux arm64 install=work (no
  hardware), the ≥ 20 GiB → 12B auto-select branch, and **cosign signature
  verification — skipped, cosign is not installed on the validating box**.
  SHA-256 was verified for every archive that was installed.
- **crates.io: `inferd-proto` + `inferd-client` 0.6.1 published** (by hand,
  same day). The release run's `Publish crates.io` job had reported success
  *without publishing* — `CARGO_REGISTRY_TOKEN` is unset and the job no-ops
  with a notice by design, so a green run does not mean the crates shipped.
  Published from a tree verified identical to the `v0.6.1` tag for both crate
  directories and confirmed against the registry API; `cargo add
  inferd-client` now resolves to 0.6.1. The misleading job has since been
  removed rather than given a secret — see Removed above; publishing is
  manual by design.

## [0.6.1] - 2026-08-02

Post-GA code review of the v0.6.0 line (correctness / dead code /
over-engineering sweep). Two security fixes, two engine fixes, dead-code
removal, and the doc drift the review surfaced.

No wire change: the v2 generation and embed surfaces are untouched
(ADR 0021 / 0017), so `wire_version` does not move and v0.6.0 clients
interoperate unchanged. The two new `inferd-proto` consts and the new
daemon flag are additive. One behaviour change is deliberate and
observable: a request declaring more than 32 attachments, or more than
128 MiB of them, is now rejected where it was previously accepted.

#### Added

- **Per-request attachment bounds on the v2 generation wire**
  (THREAT_MODEL F-1). The 64 MiB frame cap bounds one *frame*, not one
  *request*: each declared attachment entitles the sender to one further
  BLOB frame, so an unbounded attachment table multiplied a single
  in-cap request frame into unbounded reads. Two new caps in
  `inferd-proto::v2::attachment` close it —
  `MAX_ATTACHMENTS_PER_REQUEST` (32) and
  `MAX_ATTACHMENT_BYTES_PER_REQUEST` (128 MiB).
  `RequestV2::resolve()` enforces the count so every producer and
  non-streaming consumer sees the same contract, and the daemon's
  `lifecycle_v2::read_attachment_blobs` enforces both while streaming —
  charging the byte budget against the **declared**
  `BlobDescriptor::len` before reading the payload, so an over-budget
  request costs the daemon no allocation. Over-count is
  `invalid_request`; over-budget is `frame_too_large`. Both caps are
  `const`-asserted to stay at or above `inferd-http`'s own limits, so
  the daemon cannot refuse what the bridge legitimately builds. Covered
  by new proto tests, daemon tests against small injected bounds, and a
  Tier 5 case (`f1_attachment_table_is_bounded_per_request`).
- **`--write-timeout-secs` / `INFERD_WRITE_TIMEOUT_SECS`** (default 60)
  bounds how long the daemon will block writing one response frame
  before it abandons the frame and drops the connection. Operator escape
  hatch: `0` disables the bound, which the daemon logs a warning about at
  startup. See the fix below for why the bound exists.

#### Fixed

- **A peer that stops reading can no longer hold an admission permit
  indefinitely** (THREAT_MODEL F-17). Response writes happen downstream
  of the admission gate, while the request's permit is alive: a local
  client that sent a valid request, got admitted, and then simply stopped
  draining its socket filled the kernel send buffer and blocked the
  daemon inside `write_all` forever — holding a generation slot with no
  timeout to break it. `active_permits + queue_depth` such connections
  (11 by default) starved generation for every other consumer on the
  machine, at almost no cost to the attacker. Both surfaces now bound
  every response write (`lifecycle_v2::write_response_v2`,
  `lifecycle_embed::write_response_embed`) — they share one `Admission`,
  so an unbounded embed write starved generation too. The bound covers
  the writer-mutex acquisition as well as `write_all`/`flush`, since the
  wedged task holds that lock. Regression test
  (`tests/write_stall.rs`, UDS + named pipe) fails with the bound
  removed: the victim client is refused `queue_full` on every one of
  ~184 attempts across a 20s budget, versus served in under a second
  with the fix.

- **Double `llama_sampler_accept` on the grammar path**
  (`inferd-engine::llamacpp`). `llama_sampler_sample` accepts into the
  chain internally, so the unconditional post-sample accept advanced
  every stateful chain member twice on the non-grammar path. The two
  paths genuinely differ in who accepts — the manually-applied grammar
  sampler is deliberately kept out of the chain — so the call site now
  reports which case it took (`chain_needs_accept`) instead of leaving a
  latent double-advance for any future penalties / dry sampler.
- Hoisted the vocab-sized candidate buffer out of the grammar path's
  per-token loop: one allocation per generation instead of one per
  token, refilled in place.

#### Removed

- Dead code in the llamacpp adapter, all compiler-proven unreachable and
  previously silenced with `#[allow(dead_code)]`:
  `mtmd::InputChunk::raw()` (its doc claimed a caller that never
  arrived) and `BackendCapabilitiesV2::audio_sample_rate`. The live
  `InputChunks::raw()` / `Bitmap::raw()` are untouched.

#### Changed

- Doc drift the review turned up. `THREAT_MODEL.md`: F-1 rewritten (it
  claimed the per-frame cap covered heap exhaustion — true per frame,
  false per request), F-7 now names the real per-surface accept events
  (`v2_connection_accepted` / `embed_connection_accepted`, not
  `connection_accepted`), F-8 moved from *mitigated* to *n/a — closed by
  removal* since ADR 0022 deleted the TCP endpoint and its `auth.rs`
  shared-key compare in v0.5.0. Same removal swept out of `CLAUDE.md`
  (which also listed a nonexistent `auth.rs`), `context.md`,
  `INTEGRATING.md` (documented a `--tcp` flag that no longer exists),
  `docs/ai.internals.explained.md`, and `packaging/README.md`. The
  per-request attachment bounds are now normative in
  `docs/protocol-v2.md` §3.7.

## [0.6.0] - 2026-07-13

GA promotion of the `0.6.0-rc.1`…`rc.4` line (below). Headline changes
over v0.5.1: vendored **llama.cpp `b9850`** + Gemma 4 **12B** support,
**boot-time model auto-selection** (ADR 0023), and the **`inferd-http`
OpenAI-compat bridge** (ADR 0020) — now with **vision** (`image_url` →
RGB attachment) and **structured output** (`response_format` → grammar),
bundled in every tarball. Plus Go-client fixes (nested-module version
tags #48, `DialPipe` busy-retry #49) and a daemon backend-init log fix
(#47). Windows arm64 is **parked** for this line (task #185 — the b9850
`libomp` load crash); it ships on Linux x86_64 (CUDA), Linux arm64,
macOS arm64 (Metal), and Windows x86_64 (CUDA).

**install=work validated on all three shipping-desktop platforms** from
the signed rc.4 tarballs (full G1–G6: text, thinking-no-leak, grammar,
vision, bridge, bridge-vision+grammar):

- **Windows x64 CUDA** (RTX 5080): upgrade-over-running install → all gates
  green; auto-select → E4B (16 GiB < 20 GiB).
- **Linux x64 CUDA / WSL** (clean env): fresh install **auto-fetched the
  models over HTTPS** (ADR 0010 bootstrap) → all gates green.
- **macOS arm64 Metal** (16 GiB, issue #50): full G1–G6 green,
  `accelerator=metal`; auto-select → E4B.

The **12B auto-select tier ships documented-unvalidated**: the selection
logic is unit-tested and the E4B path is proven, but no `>= 20 GiB`
accelerator was available to load a real 12B model. See
`docs/v0.6-validation.md`.

## [0.6.0-rc.4] - 2026-07-13

### Added

- **Structured output through the `inferd-http` bridge.** OpenAI
  `response_format` `{"type":"json_schema","json_schema":{"schema":{…}}}`
  now maps to the daemon's grammar-constrained decoding, so bridge output
  conforms to the requested JSON Schema (the gap was found during
  install=work validation — the field was silently dropped — and
  SDK-verified after the fix). `text` / `json_object` stay unconstrained
  by design.

### Fixed

- **Go client version pinning** — `clients/go` is a nested module, so Go
  resolves its versions only from path-prefixed tags (`clients/go/vX.Y.Z`);
  the repo tagged only root releases, so `go get …/clients/go@vX.Y.Z`
  failed and only commit pseudo-versions resolved. The release workflow
  now publishes a matching `clients/go/<version>` tag per release (#48).
- **Go client `DialPipe` busy-retry** (Windows) — the busy-pipe match was
  case-sensitive and missed the capitalised OS error ("All pipe instances
  are busy."), so the retry never fired and a busy pipe failed
  immediately; now case-insensitive (#49).
- **Backend init failures now log to the NDJSON activity log before the
  daemon exits** (issue #47). Previously, a failed backend/model load
  (e.g. `mmproj_image_max_tokens` below the model's vision floor, or any
  other `LlamaCpp::new()` failure) only wrote the error to stderr.
  In background-service mode (systemd / launchd) stderr is invisible;
  `inferdctl doctor` reported only "daemon not running" with no
  actionable cause. The error is now emitted via `error!()` through the
  tracing stack — landing in `~/.inferd/logs/inferd.ndjson` — before the
  daemon shuts down admin and exits.

## [0.6.0-rc.3] - 2026-07-11

### Changed

- **Windows arm64 (`aarch64-pc-windows-msvc`) temporarily parked** from the
  release matrix (tracked in the issue for the b9850 arm64 load crash).
  On the b9850 line the arm64 daemon crashes at process load
  (`0xC0000135`) even with `libllama` + `ggml` + `libomp` staged next to
  the exe — b9850 introduced another load-time DLL dependency still being
  pinpointed. arm64 was build+sign-only in v0.5.1 (never runtime-validated
  on Windows-on-ARM) and has no CUDA path, so it does not block the
  release. v0.6.0 ships on Linux x86_64 (CUDA), Linux arm64, macOS arm64
  (Metal), and Windows x86_64 (CUDA). The `libomp.dll` staging fix and the
  arm64 build steps are retained for a clean re-add.

## [0.6.0-rc.2] - 2026-07-11

### Fixed

- **Windows arm64 daemon crashed at load (`0xC0000135`).** llama.cpp
  b9850 moved OpenMP linkage into `ggml-base` (the core lib in the startup
  import chain); the arm64 clang-cl build's OpenMP runtime is LLVM's
  `libomp.dll`, which — unlike x64's system `vcomp140.dll` — was never
  staged next to the binary. `build.rs` now stages `libomp.dll` into
  `backends/` for the windows-aarch64 target (mirroring the Linux
  CUDA-runtime bundling pattern), and `release.yml` verifies its presence.
  This failed the v0.6.0-rc.1 release build's arm64 leg; rc.1 published no
  artifacts.

## [0.6.0-rc.1] - 2026-07-11

### Added

- **`inferd-http` — OpenAI-compatible HTTP bridge** ([ADR 0020](docs/adr/0020-inferd-http-bridge-is-a-separate-process.md)
  Surface A). A new, separate, user-launched binary crate that exposes
  `/v1/chat/completions` (streaming + non-streaming), `/v1/embeddings`
  (`float` + `base64`), `/v1/models`, and `/health` over localhost, and
  translates them to the daemon's native v2/embed IPC via `inferd-client`.
  Point OpenCode or any OpenAI-SDK client at it. The daemon is unchanged
  and serves no HTTP (ADR 0006/0022); the bridge is a consumer (ADR 0014).
  Localhost + no-auth by default; a non-loopback bind requires `--token`
  (bearer). Each request dials a fresh daemon connection so the admission
  queue multiplexes and client-disconnect cancels the job. Verified
  end-to-end with the real `openai` Python SDK (chat stream + non-stream,
  embeddings string + batch, models). The OpenAI Chat/Embeddings wire
  structs were extracted to a new shared **`inferd-openai-wire`** crate so
  the outbound `openai-compat` adapter and this inbound bridge share one
  canonical definition and cannot drift.
- **Vision through the `inferd-http` bridge.** The bridge now accepts
  OpenAI multimodal chat content — a `user` message whose `content` is an
  array of `text` / `image_url` parts. It decodes the image (base64
  `data:` URL, PNG/JPEG) to raw interleaved RGB and forwards it as an
  inferd image attachment over the BLOB-frame wire (the daemon links no
  image codec — ADR 0016 — so the consumer decodes). Verified end-to-end
  with the real `openai` SDK against a live vision daemon: a known-text
  image round-trips and Gemma 4 transcribes it (stream + non-stream).
  **Security:** only `data:` URLs are accepted — a remote `http(s)://`
  image URL is refused with a 400 (no server-side fetch → no SSRF); decode
  is bomb-guarded (8 MiB encoded cap, 4096²/48 MiB per-image, 64 MiB max
  decoder allocation) and a request is bounded to 8 images / 128 MiB
  aggregate decoded RGB. `MessageContent` (the message `content` field) is
  now a string-or-parts type shared with the outbound adapter; text-only
  requests are byte-identical to before.

### Documentation

- **Consuming inferd across a VM/container boundary**
  (`docs/consuming-across-a-boundary.md`, [ADR 0024](docs/adr/0024-wsl-relay-for-containerized-middleware.md)).
  inferd ships no cross-VM bridge: reaching the daemon's IPC endpoint from
  another memory domain (e.g. WSL2/container middleware → Windows-host
  daemon) is the consumer's concern. Consumer-owned bridging preserves
  per-app identity/mapping fidelity (a shared relay would collapse every
  caller to one identity at the daemon's peer-cred boundary). Documents the
  validated options (co-locate the daemon — recommended, zero compromise;
  `inferd-http` bridge; localhost-forwarding relay) and the proven dead
  ends (interop-stdio ~512 KiB bulk cap, third-party Hyper-V sockets are
  privilege-walled on WSL2). Records that no supported + no-TCP + cross-VM
  bulk transport exists on WSL2.

### Added

- **Boot-time model auto-selection by accelerator memory**
  ([ADR 0023](docs/adr/0023-boot-time-model-auto-selection-by-accelerator-memory.md)).
  Opt in with `model_autoselect: "auto"` in `~/.inferd/config.json`: at
  boot the daemon picks the Gemma 4 generation variant from the chosen
  accelerator's **total** memory — `>= model_autoselect_min_vram_gib`
  (default 20 GiB) → 12B, else E4B (4B). With no `backends:` listed it
  synthesises the generation + embed backends from pinned defaults
  (zero-config); an explicit `backends:` generation entry always
  overrides. **Free** memory gates a pre-load fit check that emits a
  clear "insufficient accelerator memory" error (with remedies) instead
  of llama.cpp's cryptic GPU-OOM. The embed model co-locates on the
  accelerator unless memory is tight, then falls back to CPU
  (`n_gpu_layers = 0`) rather than failing to load. Default is `"off"` —
  fully backwards-compatible; existing pinned configs are unchanged. No
  wire change; one-warm-model (ADR 0012) preserved. Threshold backed by
  `docs/benchmarks/gemma4-e4b-vs-12b.md`.

### Changed

- **Bumped vendored llama.cpp `b9159` → `b9850`** (`5c0e94683` →
  `4f31eedb0`; 2026-05-14 → 2026-06-30, ~691 commits). Primarily to gain
  **Gemma 4 12B / "unified" variant** support (the dense
  `Gemma4UnifiedForConditionalGeneration` arch, distinct from the
  E2B/E4B variant we already ran; upstream PRs #24077/#24082/#24088,
  floor commit `94a220cd674`), plus a large body of bug fixes including
  several Gemma 4 correctness fixes. Two build-side reconciliations were
  needed for the jump: the hand-maintained mtmd model-source list in
  `cpp/CMakeLists.txt` was replaced with a `models/*.cpp` glob (it had
  drifted — `hunyuanocr.cpp` was renamed and the new `gemma4ua/gemma4uv`
  encoders were missing), and `llama_batch` was added to the mtmd
  bindgen import (b9850's `mtmd.h` references it in a callback type).
  Validated against the real `gemma-4-e4b` model on CUDA: all five
  `llamacpp-integration` tests pass (grammar, thinking activation,
  malformed-schema-no-crash, text+image multimodal round-trips). The
  v0.5.x maintenance line (`release/0.5`) stays on `b9159`.

## [0.5.1] - 2026-06-30

GA promotion of [0.5.1-rc.2] (below) — the Gemma 4 GA thinking feature
(parse + activation), the `mmproj_image_max_tokens` OCR knob, and the
Windows arm64 tarball. No code changes beyond rc.2; this tag adds the
quinn-proto lockfile bump and records cross-platform validation.

### Fixed

- **`cargo audit` advisory RUSTSEC-2026-0185** (quinn-proto remote memory
  exhaustion, high/7.5). Lockfile-only bump `quinn-proto 0.11.14 → 0.11.15`.
  `quinn-proto` is locked only because `reqwest` lists it behind its `http3`
  feature, which inferd does not enable (`default-features = false`,
  `rustls-tls`/`json`/`stream` only) — it never compiled into any shipped
  binary, so the rc.2 tarballs are unaffected. This clears the CI `cargo
  audit` gate on `main` and ships a clean lockfile into the GA tag. No code
  or feature change.

### Validation

install=work + real-model feature gates from the **signed rc.2 tarballs**
(SHA-verified, no mock). Full matrix in `docs/v0.5-validation.md`. All
three shipped accelerator targets green:

- **Windows x64 CUDA** (RTX 5080, 2026-06-30): bundled `install.ps1`
  upgrade-over-running 0.5.0→rc.2 → `status=ready`, `accelerator=cuda`,
  `wire_version=1`, `thinking=true`. Text / thinking (no channel-token
  leak) / grammar / OCR all green. Proved the `mmproj_image_max_tokens`
  knob is wired into libmtmd (sub-floor value shifts the pixel budget →
  loud mtmd failure; valid values work).
- **Linux x64 CUDA** (WSL Ubuntu, RTX 5080, 2026-06-30): stage + flatten
  `backends/*` → daemon on UDS → same four gates byte-identical green.
- **macOS arm64 Metal** (Apple Si, 2026-06-30, #46): `install-launchagent.sh`
  → `accelerator=metal`, `device=MTL0 vram=11.8 GiB`. Thinking (no leak),
  grammar, malformed-schema-no-crash, and `v2_image_attachment_round_trips`
  multimodal all green (~161s, no panic). Real embed 256-dim.

A background-mode log-visibility rough edge for sub-floor
`mmproj_image_max_tokens` values is tracked in #47 (non-blocker — fails
loudly to stderr; no realistic operator config hits it).

## [0.5.1-rc.2] - 2026-06-30

Second RC of the 0.5.1 patch line. Adds the Gemma 4 thinking feature
(parse + activation), the OCR image-token knob, and (from rc.1) the
Windows arm64 tarball + CUDA-CI Ninja fix. Cut for cross-platform
install=work + real-model verification before GA.

### Fixed

- **Gemma 4 GA thinking-token parsing.** The tool/thinking parser
  detected reasoning output with `<|think|>` … `<|/think|>`, but Gemma 4
  GA emits it as `<|channel>thought` … `<channel|>` (verified against the
  released Gemma-4-E4B-It GGUF `chat_template`). `<|think|>` is only the
  *system-turn activation* token, never an output wrapper, and `<|/think|>`
  does not exist. Against the real model the old openers never matched, so
  the reasoning trace **leaked into the user-visible `text` block** instead
  of the separate `thinking` block. The parser now keys off the GA tokens
  (and treats a stray `<|think|>` in output as plain text). Tool-call
  output parsing (`<|tool_call>call:NAME{…}` with `<|"|>` string delimiters)
  was already correct and is unchanged. (The matching *activation* side
  — emitting `<|think|>` to turn thinking on — is the new `thinking`
  request field below.)

### Added

- **Thinking (reasoning) activation** — `RequestV2.thinking` (optional
  bool). When `true`, the daemon turns on the model's reasoning mode; for
  Gemma 4 the renderer injects the `<|think|>` activation token into the
  system turn (synthesising an empty system turn if the request has none),
  per the GA prompt-format spec + the released GGUF `chat_template`. The
  reasoning trace comes back on `thinking` response blocks (separated by
  the parser fix above), not user-visible `text`. Omitted/`false` is
  behaviour-preserving; additive — no `wire_version` bump. Backends
  without reasoning support ignore it. Plumbed through proto
  (`RequestV2`/`ResolvedV2`), the Gemma 4 renderer, and the Go client
  (`Thinking *bool`). Documented in `protocol-v2.md` §3.2.
- **`mmproj_image_max_tokens` config knob** for llamacpp backends
  (issue #42). Caps image tokens per image for dynamic-resolution vision
  models (Gemma 4); a higher value reduces the projector's downscaling so
  small / sparsely-spaced text (OCR of fine print, leader-dotted lines)
  survives, at the cost of more tokens and slower encode. Maps to
  libmtmd's `image_max_tokens`; set at mtmd init (a context property, not
  per-request). Omitted/`null` reads the model metadata default —
  behaviour unchanged. Documented in `docs/protocol-v2.md` §3.5 (image
  preprocessing is daemon-owned per ADR 0013, so there is deliberately no
  per-request wire knob; operators tune the budget, consumers
  pre-segment/upscale for higher fidelity).

## [0.5.1-rc.1] - 2026-06-25

First RC of the 0.5.1 patch line: adds the Windows arm64 release tarball
and fixes the Windows CUDA CI drift. Cut so the never-before-CI'd arm64
Windows build path can be exercised on a real arm64 Windows machine
(release.yml is tag-triggered; this is its first run).

### Added

- **Windows arm64 release tarball** (`aarch64-pc-windows-msvc`). Built
  natively on GitHub's GA `windows-11-arm` runner (not a cross-compile)
  and added to the release matrix — so releases now ship 5 targets:
  Linux x86_64 + arm64, macOS arm64, Windows x86_64 + arm64. The arm64
  Windows build is `dl-backends` (CPU ggml variants; no CUDA on Windows
  arm64), uses the default VS CMake generator (cmake-rs targets ARM64 via
  `-A ARM64`), and bundles the same arch-agnostic `install.ps1`. The
  publish job's asset-completeness check was bumped 4→5.

### Changed

- **CI: Windows CUDA release build switched to the Ninja CMake generator**
  (`inferd-engine/build.rs`) AND kept pinned to `windows-2022` (#162).
  Two separate VS↔CUDA couplings break this build; Ninja fixes only one:
  (1) MSBuild + CUDA's `visual_studio_integration` props are
  version-matched to a VS release → "No CUDA toolset found" — Ninja
  sidesteps MSBuild and fixes this, dropping the `visual_studio_integration`
  sub-package; (2) `nvcc`'s `crt/host_config.h` hard-`#error`s on any MSVC
  outside 2017–2022, and Ninja does NOT help (nvcc still shells out to
  cl.exe and checks its version). When `windows-latest` rolled to VS 2026
  (MSVC 14.51), coupling (2) fired ("unsupported Microsoft Visual Studio
  version"). So the image stays pinned to `windows-2022` (VS 2022, which
  CUDA 12.6 accepts); the Ninja generator + `ilammy/msvc-dev-cmd` dev-env
  remain (correct for coupling #1, harmless on the pin). Revisit the pin
  when moving to a CUDA version that supports the current
  `windows-latest` VS.

## [0.5.0] - 2026-06-24

GA promotion of `[0.5.0-rc.1]` (below) — no code changes beyond the
version bump. Validated install=work from the rc.1 release tarballs on
all three testable platforms (real `llamacpp`, no mock):

### Validation

- **Windows x86_64 CUDA (RTX 5080) — v0.5.0-rc.1 tarball validation green
  (2026-06-24):** SHA256 verified. `inferd-daemon 0.5.0-rc.1` +
  `inferdctl 0.5.0-rc.1` (backends flattened next to the exe).
  TCP-removal confirmed (`--tcp` → "unexpected argument", rejected). Real
  `llamacpp` backend on CUDA, model ready ~7s → real generate
  (`answer="Paris"`, `backend=llamacpp`, `stop=end_turn`) + grammar
  (`response_format` JSON Schema → `{"city":"Paris","population":2141000}`,
  valid JSON matching schema). Windows CUDA build clean under the new
  CUDA v13.3 toolkit (did not reopen #162).
- **Linux x86_64 / WSL Ubuntu — v0.5.0-rc.1 tarball validation green
  (2026-06-24):** SHA256 verified. Versions `0.5.0-rc.1`. `--tcp`
  rejected. Real `llamacpp` backend (CPU) over UDS → real generate
  (`"Paris"`, `backend=llamacpp`, `end_turn`) + grammar
  (`{"city":"Paris","population":2141000}`, valid JSON). Config
  validation correctly rejected a non-`https` `source_url` along the way
  (expected hardening).
- **macOS arm64 Metal — v0.5.0-rc.1 tarball validation green (2026-06-24):**
  SHA256 verified. `inferd-daemon 0.5.0-rc.1` + `inferdctl 0.5.0-rc.1`.
  TCP-removal confirmed (`--tcp` rejected: "unexpected argument"). Installed
  via `install-launchagent.sh` → `status=ready`, `wire_version=1`,
  `accelerator=metal`, `device=MTL0 vram=11.8 GiB`. Real embed (256-dim).
  Grammar tests (`grammar_llamacpp`, `llamacpp-integration`):
  `response_format_constrains_output_to_json` → `{"city":"Paris","population":2141000}` ✅;
  `malformed_schema_errors_does_not_crash` → clean error, daemon survives ✅.
  Both in 12.71s. No panic, no abort. See issue #40.

## [0.5.0-rc.1] - 2026-06-24

**Breaking: the daemon binds no inbound network listener.** Inbound
loopback TCP — deprecated in 0.4.0 ([ADR 0022](docs/adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md))
— is removed. The daemon is reachable only over its local Unix domain
socket (Unix) / named pipe (Windows), authenticated by kernel-attested
peer credentials (THREAT_MODEL F-7). Anything needing network access
goes through the separate `inferd-http` bridge ([ADR 0020](docs/adr/0020-inferd-http-bridge-is-a-separate-process.md),
Surface B). Cut as 0.5.0 (not 0.4.1) because removing the published
client TCP constructors is a breaking API change and Cargo treats
0.4.x as compatible. (ADR 0022's body says "v0.4.1"; superseded by this
release's actual versioning — the ADR is immutable so its text stands as
the decision-time record.)

### Added

- **Structured output (`response_format`)** — a `RequestV2` may carry an
  optional `response_format: { type: "json_schema", schema: {...} }`
  (additive; no `wire_version` bump). The daemon shapes this
  model-agnostic JSON Schema to the engine (ADR 0013 gateway): the
  llamacpp backend compiles it to GBNF (`json_schema_to_grammar`) and
  installs a grammar sampler, so generated output is guaranteed to be
  valid JSON conforming to the schema. The grammar sampler is kept
  separate from the sampler chain and applied per-token
  (apply-grammar → apply-chain → accept), mirroring llama.cpp's
  `common_sampler` — chaining it would throw across FFI. A malformed
  schema fails closed (error frame), never crashes the daemon. Verified
  on a real model: `{"city":"Paris","population":2141000}` from a
  city/population schema. Cloud-backend pass-through (openai/bedrock
  `response_format`) is a follow-up; 0.5.0 ships the llamacpp path.

### Removed

- **Daemon:** `--tcp` / `INFERD_TCP`, `--embed-tcp` / `INFERD_EMBED_TCP`,
  and `--api-key` / `INFERD_API_KEY` flags; the `endpoint::bind_tcp`
  listener + `DEFAULT_TCP_ADDR`; the `serve_tcp_v2` / `serve_tcp_embed`
  loops; the first-frame `{"type":"auth","key":...}` TCP auth path and
  the entire `auth.rs` module (AuthFrame + constant-time key compare);
  the `tcp`/`tcp_embed`/`api_key_env` `ListenConfig` fields;
  `PeerIdentity::from_tcp` + its `remote_addr` field;
  `AcceptContext::expected_api_key`.
- **Rust client (`inferd-client`):** `ClientV2::dial_tcp` and
  `EmbedClient::dial_tcp` (breaking — the reason for the minor bump).
- **Go client:** `DialTCP` (breaking).

### Changed

- Daemon transport selection is now UDS (Unix) / named pipe (Windows)
  only; `require_one_transport` and the platform error messages no
  longer mention `--tcp`.
- Integration tests re-homed off the TCP harness onto UDS
  (`serve_uds_v2`) — `v2_stub` (incl. the W2 `wire_version`-mismatch
  gate), `stress`, `queue_full`, `logx` (incl. the secret-redaction
  security test), `echo_llamacpp`; the Go end-to-end test now dials the
  per-test named pipe (Windows) / UDS (Unix). No test coverage was
  dropped — only the harness transport changed.

## [0.4.0] - 2026-06-23

The v0.4 line: a **unified IPC wire format** ([ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)).
One generation API (v1 folded into v2, the v1 socket + types removed),
length-prefixed type-tagged framing replacing newline-delimited JSON,
media carried as raw BLOB frames keyed by `attachment_id` instead of
base64-in-JSON, and an in-band `wire_version` that fails loudly on
mismatch (`wire_version_unsupported`). The full set of changes,
removals, and fixes is itemised in the `[0.4.0-rc.1]`…`[0.4.0-rc.3]`
sections below; this section ratifies that cumulative work. Between
`0.4.0-rc.3` and `0.4.0`, beyond the version bump, the only changes are
the documentation sweep, the `inferd-http` / no-network-listener
decision (ADR 0022, docs + deprecation notes only — no runtime behaviour
change), and two additive client conveniences; all itemised below.

### Added

- **`docs/protocol-v2.md`** — a normative, self-contained wire-protocol
  specification (framing byte-layout, message catalogue, closed error
  set, worked example, client-author invariants) so consumers can
  implement middleware from the document rather than from sample code.
  Validated by building a fresh client against the spec alone.
- **Go client `DialInfer(ctx)`** — a portable, transport-agnostic dialer
  for the platform-default generation socket (UDS on Unix, named pipe on
  Windows), replacing TCP as the cross-platform one-liner in the docs.
- **Go client `ErrV2WireVersionUnsupported`** — the `wire_version_unsupported`
  error code was missing from the Go `ErrorCodeV2` constants; a Go
  consumer could not name-match the v0.4 handshake's signature error.
- **`inferdctl` crate README** — was absent (the crate shipped to
  crates.io undocumented); now present and wired via `readme`.

### Deprecated

- **Inbound loopback TCP in the daemon** ([ADR 0022](docs/adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md)).
  The daemon binds no inbound network listener; `--tcp` / `INFERD_TCP`,
  the first-frame `{"type":"auth"}` API-key path, and the client
  `dial_tcp` / `DialTCP` constructors are **deprecated in v0.4.0 and
  scheduled for removal in v0.4.1**. They remain in v0.4.x purely so the
  cross-platform test harness keeps working; they are removed from all
  user-facing surfaces (spec, READMEs, site, sample clients). Network
  access is the separate `inferd-http` bridge's job ([ADR 0020](docs/adr/0020-inferd-http-bridge-is-a-separate-process.md),
  Surface B). This supersedes ADR 0009's loopback-TCP clause and
  resolves ADR 0020's open question (option b).

### Documentation

- Refreshed all crate + client READMEs to v0.4 (version strings, the
  removed v1 `generate` trait method, NDJSON→length-prefixed framing,
  the canonical Go-client pointer for py/ts stubs).
- GitHub Pages: added a "no network listener, even loopback" entry to the
  "what it isn't" list; v0.4.0 version strings.

### Validation

install=work + wire e2e validated from the **release tarballs** on every
shipped target (`docs/v0.4-validation.md`), all on real hardware:

- **Windows x86_64 (CUDA, RTX 5080):** `install.ps1` from the tarball →
  `status=ready`, `wire_version=1`, `accelerator=cuda` → real generate +
  embed. Both upgrade-over-prior-install and fresh-from-nothing
  (auto-pull ~6 GB → ready) paths.
- **Linux x86_64 (CUDA, RTX 5080 via WSL):** same, upgrade + fresh.
- **Linux x86_64 (CPU, no-GPU container):** ADR 0019 probe falls back to
  `accelerator=cpu` on a GPU-less host → real generate + embed.
- **macOS arm64 (Metal, Apple M1):** `install-launchagent.sh` from the
  tarball → `accelerator=metal` → real generate + embed; W4 multimodal
  (image through the mtmd BLOB path, no base64) green.
- **Gate 2 wire** (W1 cross-language Go round-trip, W2 `wire_version`
  mismatch fails loudly, W3 real-model text, W4 raw-BLOB multimodal)
  green across the platforms above.

Three Windows-installer install=work bugs were found and fixed by
running the real install path from rc.2 tarballs (stale `--v2` flags,
upgrade-over-running-daemon, uninstall-before-handle-release); rc.3
shipped the fixes and re-proved install from its own tarballs.

## [0.4.0-rc.3] - 2026-06-22

Third v0.4 release candidate. rc.2 built green on all 4 platforms, but
running the real install=work path from its tarballs surfaced three
Windows installer bugs (the shipped rc.2 Windows installer would fail to
start a fresh daemon). rc.3 carries the fixes so the *shipped* installer
is correct, and must re-prove install=work from its own tarballs before
GA.

### Fixed

- **`packaging/windows/install.ps1`: dropped the removed `--v2` /
  `--v2-addr` flags and pointed `--pipe` at the neutral `\\.\pipe\inferd`
  path** (the v0.4 consistency sweep fixed the systemd unit + launchd
  plist but missed the Windows installer). The rc.2 installer launched a
  daemon with flags v0.4 no longer accepts, so a fresh Windows install
  failed to start.
- **`install.ps1`: stop the running daemon *before* staging the binary**
  — Windows holds an exclusive lock on a running `.exe`, so an
  upgrade-over-running install failed at the copy step.
- **`uninstall.ps1 -Purge`: wait for the killed daemon to release its DLL
  handles before deleting the install dir** — otherwise the recursive
  delete hit "Access denied" on `cublas64_12.dll`.

All three were found by running the actual install/upgrade/uninstall
path from the rc.2 tarballs on Windows + WSL (RTX 5080); the v0.4 wire +
fresh-from-nothing auto-pull (~6 GB → ready → real generate + embed) were
validated on both. See `docs/v0.4-validation.md`.

## [0.4.0-rc.2] - 2026-06-18

Second v0.4 release candidate. rc.1's release run failed: the Windows
x86_64 CUDA build broke in CMake `enable_language(CUDA)` with "No CUDA
toolset found" — `windows-latest` now resolves to `windows-2025`
(VS 2026 / MSVC 19.51), and CUDA 12.6's `visual_studio_integration`
only registers MSBuild props for VS 2022. Because `publish` needs all
four platform builds, no release page or tarballs were produced.

### Fixed

- **Windows CUDA release build pinned to `windows-2022`** (VS 2022 /
  MSVC 19.4x) instead of `windows-latest`. CUDA 12.6's Visual Studio
  integration doesn't support the VS 2026 toolchain that
  `windows-latest` (→ `windows-2025`) now ships, so
  `enable_language(CUDA)` failed at cmake-configure time. Pinning the
  older runner image keeps the CUDA→MSVC toolset path working. Revisit
  when a CUDA release ships VS 2026 integration. The 613 MB Linux
  x86_64 artifact is benign — it's the bundled cuBLAS/cuBLASLt CUDA
  redist libs (NVIDIA-EULA-permitted) a CUDA tarball legitimately ships
  next to `libggml-cuda.so`.

## [0.4.0-rc.1] - 2026-06-18

First v0.4 release candidate — cut to produce signed per-platform
tarballs for the install=work validation gate (ADR 0021 wire redesign).
**Release run failed** (Windows CUDA build, see rc.2); no tarballs
published. Not GA: the Gate-1 tarball-install loop (download → install →
auto-pull → real generate + embed) has not yet been run from any
artifacts, and CUDA/GPU paths are unvalidated for v0.4. See
`docs/v0.4-validation.md`.

### Fixed

- **`defaultDaemonBin` in Go e2e test now prefers `target/release` over
  `target/debug`** (`clients/go/client_test.go`). On macOS, a stale
  `target/debug/inferd-daemon` (rc.12, NDJSON) was being used instead of
  the v0.4 release binary (LP framing), causing `TestEndToEndAgainstDaemon`
  to return `code=internal` and the rc.12 daemon to collide with the launchd
  daemon's `${TMPDIR}/inferd/inferd.sock` (crashing its accept loop). Fixed
  by checking `target/release` first. Test also gains `HOME`/`USERPROFILE`
  isolation to prevent the test daemon from touching real `~/.inferd/`.

### Validation

- **v0.4 Gate 1 + W1 + W3 validated on macOS arm64 Metal (Apple M1,
  2026-06-17).** `inferdctl doctor` reports `wire_version=1`,
  `accelerator=metal`, `device=MTL0 vram=11.8 GiB`, generation socket
  `${TMPDIR}/inferd/inferd.sock`. `go test ./...` (15 tests) green.
  Real-model LP generate: `answer="Four"`, `backend=llamacpp`,
  `stop=end_turn`. W4 (BLOB multimodal) blocked on mmproj build; wire
  encoding verified by mock tests.

- **v0.4 framing proven end-to-end** (ADR 0021 / #34). Automated:
  `clients/go` `TestEndToEndAgainstDaemon` round-trips `GenerateV2`
  against a freshly-built v0.4 mock daemon over the length-prefixed
  wire. Real model (this box, llamacpp): the migrated Go client sent a
  text request → *"Hello there friend."* (in=17/out=4) and a 256×256
  image as a raw BLOB frame → *"A solid red circle is centered on a
  white background."* (in=276 — the image expanded into ~250 vision
  tokens through mtmd, no base64). Confirms the full pipeline: LP
  request + BlobDescriptor + BLOB frames → wire_version check → BLOB
  reassembly by id → raw RGB to mtmd → LP response frames.
- **v0.4 release-gate doc** added at `docs/v0.4-validation.md`: two
  gates (install=work coverage matrix re-reset for the socket/framing
  change + a wire-format end-to-end gate covering the LP round-trip, the
  `wire_version` mismatch failure, and the raw-BLOB multimodal path),
  plus the pre-tag release checklist. The proof above is recorded as the
  Windows x86_64 rows; other targets are ☐ pending release-tarball runs.

### Changed

- **v0.4 (breaking): unifying the IPC wire format** per [ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)
  (issue #34). One generation API (v1 folded into v2, v1 socket
  removed); length-prefixed, type-tagged framing (uvarint len + 1 type
  byte: `0x01` JSON / `0x02` BLOB) replacing newline-delimited JSON;
  media rides as raw BLOB frames instead of base64-in-JSON
  (`AttachmentV2.bytes` removed); in-band `wire_version` on the request
  + capabilities frame so mismatches fail loudly. Pre-launch break
  (only first-party consumers exist): the v0.3.0 crates/clients keep
  working against a v0.3 daemon but do **not** interoperate with v0.4.
  ADR 0021 supersedes parts of ADRs 0008/0009/0015; the post-launch
  freeze posture returns after v0.4.

### Fixed

- **Daemon no longer silently falls back to the mock backend** (GA
  hardening). Previously, with no `--backend` flag and no usable config
  (missing / unreadable / declares no backends), `build_backends`
  returned the in-memory `Mock` — a real install could serve fake
  `"mock-response"` tokens instead of failing. inferd now **refuses to
  start** with an actionable error in that case; the mock backend is
  reachable only via an explicit `--backend mock`. This restores the
  install=work guarantee (a mock-default install is a release blocker).
  Verified live: a feature-built daemon with an empty-backends config
  exits with "refusing to start: no usable inference backend".
- **Real-model generation was broken in v0.4** — two regressions from the
  v1→v2 fold, both caught by running `docs/v0.4-validation.md` Gate 2 W3
  (real-model text e2e) on Windows; neither was covered by the existing
  mock-backend tests. (1) `LlamaCpp::capabilities()` only set `v2: true`
  when an mmproj had loaded, so a text-only generation backend advertised
  `v2: false` and the daemon's v2-capability gate refused *every* request
  with `"backend does not advertise v2 capability"`. `v2` is now `true`
  for any llamacpp generation backend (it always allocates a generation
  context); `vision`/`audio` still track the mmproj probe. (2)
  `run_generation_v2` unconditionally required an mtmd context
  (`NoMmproj` error), so text-only generation aborted mid-stream with no
  terminal frame. It now branches: mtmd path when an mmproj is present
  (required for attachments), plain tokenise + `llama_decode` prefill for
  the text-only case (restoring what the removed v1 path did). Added the
  `unsupported_wire_version_errors_and_closes` integration test (Gate 2
  W2) — the `wire_version` gate had no through-the-socket coverage.
- **v0.4 consistency sweep across clients, packaging, CI, and docs**
  (ADR 0021 / #34). Caught install=work-breaking leftovers the wire
  change left behind: the Go client's `DefaultInferAddr()` returned the
  old `infer.sock` / `infer.v2.sock` paths (a consumer using the
  default would dial a socket the daemon no longer binds) — now
  `inferd.sock` / `\\.\pipe\inferd`; the systemd unit and launchd plist
  passed the removed `--v2` / `--v2-addr` flags and the stale
  `infer.sock` path (daemon would fail to start) — now the neutral
  socket with `--embed` only; the CI install-smoke checked for
  `infer.sock` / `infer.v2.sock` and sent raw NDJSON to the generation
  socket — rewritten to assert `inferd.sock` and drive the
  length-prefixed v2 wire. Also mirrored the Rust v1 excision in the Go
  client (removed the v1 `Generate` + types, kept the shared `Role` /
  `Client` core) and removed the dead `--v2` / `--v2-addr` / `--v2-tcp`
  CLI flags and `listen.tcp_v2` config knob (nothing read them).
  Documentation rewritten to match: `CLAUDE.md`, `context.md`,
  `README.md`, `INTEGRATING.md`, the `inferd-proto` / `inferd-client` /
  Go-client READMEs, the GitHub Pages site (`site/index.html`), and
  `docs/{ai.internals.explained,test-strategy}.md`; `docs/protocol-v1.md`
  gained a "HISTORICAL" banner pointing at ADR 0021.

### Removed

- **Dead v1 wire path excised** (ADR 0021 / #34). With v1 folded into
  v2 there is no v1 code left to keep alive: deleted the v1 proto types
  (`Request`/`Response`/`Resolved`/`Role`/`Message`/`StopReason`/
  `Usage`/`ImageTokenBudget`) and the `request.rs`/`response.rs`
  modules; removed the `Backend::generate(Resolved)` method,
  `TokenEvent`/`TokenStream`, and every adapter's v1 generate impl
  (`generate_v2` is now the single required generation method); dropped
  the daemon's v1 `serve_tcp`/`serve_uds`/`serve_named_pipe` +
  `handle_connection` and the GBNF `validate_grammar` guard (v2 has no
  grammar field); removed the v1 `inferd_client::Client`. The v1
  `wire.rs` proto test and v1 fuzz targets were replaced with
  length-prefixed / v2 equivalents (`lp_frame_reader`,
  `v2_request_resolve`); all daemon integration tests were migrated onto
  the length-prefixed v2 wire via a shared `tests/common` framing
  helper. `net −2156/+608` lines across 27 files; full `fmt` + `clippy
  -D warnings` + `test --all` + `audit` cycle green.

## [0.3.0] - 2026-06-03

First stable v0.3 release. Headline: **runtime accelerator detection**
(ADR 0019) — one binary ships every ggml backend as a loadable module
and picks the strongest available (Metal / CUDA / ROCm / Vulkan / CPU)
at boot — and **multimodal by default**, with the reference Gemma 4
model pulling its vision projector on first boot so a fresh install
answers questions about images with no extra config.

install=work validation is complete on every shipped target with real
hardware: Windows x86_64 CPU + CUDA (RTX 5080), Linux x86_64 CPU + CUDA
(RTX 5080 / WSL), and macOS arm64 Metal (Apple M1) — each a
fresh-machine install that auto-pulls the models and serves real
generate + embed + a real v2 image round-trip. See
`docs/v0.3-validation.md`. ADR 0019 is accepted.

This section ratifies the cumulative work detailed in the
`0.3.0-rc.1` … `0.3.0-rc.13` entries below; no code changes between
`0.3.0-rc.13` and `0.3.0` beyond the version bump.

## [0.3.0-rc.13] - 2026-06-03

### Fixed

- **`inferdctl doctor` reported only one backend's capabilities, showing
  `vision=false` on a multimodal daemon.** Two causes: (1) the admin
  `StatusBroadcaster` retained capabilities in a single watch slot, so
  each backend's caps frame overwrote the previous; (2) the caps frame's
  `backend` field used `Backend::name()` (the *kind*, `"llamacpp"`,
  identical across entries) rather than the unique config-entry name, so
  even a keyed map collided. Now retains caps per backend (keyed by the
  config-entry label), replays one frame per backend on admin connect,
  and doctor prints a `backend:` line for each — so the vision-capable
  generate backend shows `vision=true` alongside the embed backend.
  Found by Mac Claude during rc.12 Metal validation (#32); affected all
  platforms (display-only — the daemon's actual capabilities were
  correct).

### Validation

- **v0.3.0 install=work validation complete** (`docs/v0.3-validation.md`,
  2026-06-03). All non-skip rows in the coverage matrix are ☑: Linux
  x86_64 CPU + CUDA (rc.8/rc.9, RTX 5080 / WSL), macOS arm64 Metal +
  multimodal (rc.12, Apple M1). Forced-backend CPU smoke verified on
  Linux (rc.9). Phase 8 (#133) may proceed.

## [0.3.0-rc.12] - 2026-06-03

### Fixed

- **Install manifests now bind the v2 socket** (`packaging/windows/
  install.ps1`, `packaging/systemd/inferd.service`,
  `packaging/launchd/io.inferd.daemon.plist`). All three launched the
  daemon with `--pipe`/`--uds` + `--embed` but **not** `--v2`, so the
  typed-content-block surface (ADR 0015) never bound — making the
  multimodal-by-default projector (issue #30) unreachable: a consumer
  couldn't send image attachments even though the backend loaded the
  vision encoder. Added `--v2` + `--v2-addr` to each manifest. Found by
  the rc.11 from-tarball validation, which auto-pulled the projector and
  loaded vision but had no v2 socket to dial. Verified end to end: a
  real 256×256 image sent over `\\.\pipe\inferd-infer-v2` round-trips
  through mtmd (input_tokens 276 vs ~33 text-only) and the model
  correctly describes it ("A single, bright red circle").

### Fixed

- **Panic in the v2 tool/thinking sentinel parser on a multi-byte UTF-8
  token at the buffer boundary** (`crates/inferd-engine/src/llamacpp/
  tool_parser.rs`). `safe_plain_emit_len` sliced the pending `String` by
  byte offset (`pending[n - k..]`); when the model emitted a non-ASCII
  token (emoji / CJK / accented char) whose bytes straddled the tail,
  `n - k` landed inside a char and the slice panicked, killing the v2
  generation worker. Now skips offsets that aren't char boundaries
  before slicing (both sentinels are ASCII, so a non-boundary suffix can
  never match a sentinel prefix anyway). Found by the Tier-3 v2
  multimodal test during the issue #30 image-path validation; regression
  test added.

### Added

- **Multimodal is now ON by default** (issue #30). The first-boot
  default config's `gemma-4-e4b` backend now carries an `mmproj` block
  pointing at unsloth's `mmproj-F16.gguf` (sha256 `ddf46c21…`, ~945 MB),
  which lives in the same repo as the text GGUF. A fresh install
  auto-pulls the projector as a second CAS blob and loads it through
  mtmd, so the daemon reports `vision: true` / `audio: true` and accepts
  v2 image attachments out of the box — no config editing. Verified on a
  deployed daemon: `inferdctl doctor` →
  `v2=true vision=true audio=true`. Operators who want a text-only
  daemon delete the `mmproj` block after first boot. Gemma 4 is natively
  multimodal; inferd simply never pulled the projector before.
- **Multimodal (v2 vision) is now reachable via config** (issue #30).
  The v2 multimodal engine (libmtmd bridge, `generate_v2`, image
  attachments) shipped in v0.2.0, but there was no operator-facing way
  to load a vision projector — so every daemon reported `vision: false`
  and consumers correctly concluded "no multimodal." Added an optional
  `mmproj` block to a `llamacpp` backend entry (a `ModelConfig` with the
  same `name`/`sha256`/`source_url` shape as `model`). When set, the
  daemon fetches the projector as an additional CAS blob through the
  same pinned-URL + constant-time-SHA path as the base model, hands its
  path + expected SHA to `LlamaCppConfig.mmproj_path` / `mmproj_sha256`,
  and the backend's `capabilities().vision` flips `true`. `mmproj` is
  validated (https + 64-hex sha) at config load like `model`. The
  default config stays text-only (`mmproj: None`); operators opt in. The
  projector must match the base model family. Pure config/plumbing —
  the wire protocol, FFI, and chat-templating were already in place.

### Added

- **Windows x86_64 release tarball now ships CUDA** (`.github/workflows/
  release.yml`). The Windows matrix entry builds with
  `inferd-daemon/dl-backends,inferd-daemon/cuda`, installs the CUDA 12.6
  toolkit (Jimver), stages `ggml-cuda.dll` (via build.rs), and bundles
  the CUDA redist DLLs (`cudart64_12` / `cublas64_12` / `cublasLt64_12`)
  into `backends\` next to it. The driver DLL `nvcuda.dll` is never
  bundled (EULA; resolves from System32). Windows + NVIDIA users now get
  GPU acceleration out of the box; hosts without an NVIDIA driver fall
  through to CPU at the runtime probe (ADR 0019). A verify gate fails the
  build if `ggml-cuda.dll` is missing, and an import-closure check
  asserts every imported DLL is bundled, a driver DLL, or a system DLL.
  build.rs already flipped `GGML_CUDA=ON` for Windows when the `cuda`
  feature is set; this is a CI/packaging change only.

## [0.3.0-rc.9] - 2026-06-01

### Fixed

- **`INFERD_FORCE_BACKEND=cpu` did not actually run on CPU on a GPU
  host** (`crates/inferd-engine/src/llamacpp/backend.rs`). The override
  flipped the *reported* accelerator to CPU but `LlamaCpp::new` still
  passed the configured `n_gpu_layers` (default `-1` = offload all) to
  the model loader, so on a box with a registered GPU the model loaded
  onto the GPU anyway — defeating the ADR 0019 operator escape hatch
  for benchmarking / sanity checks. Now gates the layer count on the
  chosen kind: `kind == Cpu` forces `n_gpu_layers = 0` before load and
  reports it through `build_accelerator_info`. Present since the ADR
  0019 probe first landed; found in the rc.8 RTX 5080 / WSL validation
  (`docs/v0.3-validation.md`, 2026-06-01).

## [0.3.0-rc.8] - 2026-06-01

### Fixed

- **rc.7 was not install=work on Linux: the CUDA bundling step staged
  the glibc family into `backends/`, crashing the daemon at boot**
  (`.github/workflows/release.yml`). The rc.7 BFS copy loop applied
  its system-lib allow-list only in the *final verification* pass, not
  in the *copy* loop — so `libc.so.6`, `libdl.so.2`,
  `libpthread.so.0`, `librt.so.1`, `libm.so.6`, `libstdc++.so.6`,
  `libgcc_s.so.1`, and `ld-linux-x86-64.so.2` were copied next to
  `libggml-cuda.so`. With `$ORIGIN` RUNPATH the loader prefers those
  runner-built copies over the consumer's glibc; on any host whose
  glibc differs from `ubuntu-latest` the daemon dies immediately with
  `symbol lookup error: libc.so.6: undefined symbol:
  __nptl_change_stack_perm, version GLIBC_PRIVATE`. Hoisted the
  allow-list into an `is_system_lib` predicate consulted by the copy
  loop, so the glibc family is never staged and resolves from the
  consumer's own glibc at runtime. Found in the rc.7 RTX 5080 / WSL
  validation; see `docs/v0.3-validation.md` findings 2026-06-01.
- **`docs/v0.3-validation.md` Linux install step referenced a
  non-existent path.** The tarball flattens the systemd unit to
  `packaging/inferd.service`; the checklist said
  `packaging/systemd/inferd.service`. Corrected the install step.
- **Windows: daemon allocated a visible console window on startup
  (#28).** `inferd-daemon.exe` is a console-subsystem exe, so a launch
  from the per-user Startup shortcut popped a tracing window on the
  desktop. After logging is wired to the activity log + admin pipe the
  daemon now calls `FreeConsole()` — but only when it owns the console
  (sole attached process, the shortcut/double-click case). Launched
  from an interactive shell it leaves the shared console alone so
  hand-run debugging still prints. Windows-only `#[cfg(windows)]`.

## [0.3.0-rc.7] - 2026-05-28

### Fixed

- **rc.6 release workflow's CUDA bundling missed transitive deps and
  was lied to by `ldd` via the runner's `/etc/ld.so.cache`**
  (`.github/workflows/release.yml`). Two related problems with the
  ldd-based discovery loop: (a) the GHA runner has
  `/etc/ld.so.conf.d/cuda-12-6.conf` registering the toolkit install
  dir system-wide, so ldd resolved `libcudart.so.12` through that
  path and hid it from the discovery loop — but a consumer host
  without that ld.so.conf entry would still see it missing; (b) once
  `libcublas.so.12` was bundled, ldd of `libggml-cuda.so` walked
  through the bundled lib and surfaced its `libcublasLt.so.12` dep,
  which the loop had no chance to bundle (single pass). Rewrote the
  bundling step to walk `DT_NEEDED` via `readelf -d` in BFS — doesn't
  consult ld.so.cache, doesn't lie. Closure is followed transitively,
  with three exit conditions per soname: already-bundled (skip),
  driver-skiplist (skip), system lib (allow-list of libc / libm /
  libstdc++ / etc). A second pass walks the same closure and asserts
  every soname falls into one of those three buckets, failing the
  build if anything's missing.

## [0.3.0-rc.6] - 2026-05-28

### Fixed

- **rc.5 release workflow's CUDA bundling step missed real toolkit
  deps because `LD_LIBRARY_PATH` was masking them**
  (`.github/workflows/release.yml`). Jimver/cuda-toolkit exports
  `LD_LIBRARY_PATH=/usr/local/cuda-12.6/lib64`, so `ldd
  libggml-cuda.so` on the runner happily resolved `libcudart.so.12` /
  `libcublas.so.12` through that env var and reported only
  `libcuda.so.1` as missing — but a consumer machine without that env
  would still see all the toolkit libs unresolved. Step now `unset
  LD_LIBRARY_PATH` at start so ldd reports what end users actually
  see. Also single-quoted an `echo` containing `$ORIGIN` (was failing
  `set -u`) so the post-bundle re-verify step doesn't abort with
  `ORIGIN: unbound variable`.

## [0.3.0-rc.5] - 2026-05-27

### Fixed

- **rc.4 release workflow tripped on `libcuda.so.1` in CUDA bundling**
  (`.github/workflows/release.yml`). The dynamic-`ldd`-discovery loop
  added in rc.4 found `libcuda.so.1` unresolved on the GHA runner
  (correct — there's no NVIDIA driver on the build host) and tried to
  bundle it, which failed because no toolkit ships it. `libcuda.so.1`
  is the NVIDIA driver lib: redistributing it is forbidden by NVIDIA's
  EULA, it's version-locked to the consumer's installed driver, and
  it's always provided at runtime by the driver install (e.g.
  `/usr/lib/wsl/lib/libcuda.so.1` on WSL,
  `/usr/lib/x86_64-linux-gnu/libcuda.so.1` on bare metal). Workflow
  now has an explicit skiplist (`libcuda.so.1`,
  `libnvidia-ml.so.1`) that's bypassed in the bundle loop and
  filtered out of the post-bundle ldd check. Hosts without an NVIDIA
  driver still degrade correctly: the daemon's accelerator probe
  skips the CUDA backend and the user gets CPU.

## [0.3.0-rc.4] - 2026-05-27

### Fixed

- **systemd unit failed first start with `status=226/NAMESPACE` on
  fresh installs** (`packaging/systemd/inferd.service`).
  `ReadWritePaths=%h/.local/share/models %h/.inferd` requires both
  paths to exist before namespace setup; on a fresh box neither does
  yet (the daemon normally creates them on first boot), so the unit
  aborted before `ExecStart` ran. Added `ExecStartPre=/usr/bin/mkdir
  -p %h/.inferd %h/.local/share/models` so the unit is self-sufficient
  on first start. mkdir -p is idempotent and runs unprivileged under
  the user instance.
- **Linux x86_64 release tarball shipped `libggml-cuda.so` without
  its CUDA runtime deps** (`.github/workflows/release.yml`).
  `ldd backends/libggml-cuda.so` on the consumer machine showed
  `libcudart.so.12 => not found` and `libcublas.so.12 => not found`,
  and ggml's `dlopen()` swallows the missing-deps failure silently —
  so the v0.3 runtime accelerator probe registered only `Cpu` and the
  daemon ran with `gpu_layers=0` on every NVIDIA host that had no
  system-wide CUDA install. The release workflow now bundles the
  required CUDA runtime libs into `backends/` next to the MODULE,
  forces `DT_RUNPATH=$ORIGIN` on libggml-cuda.so via patchelf
  (idempotent), and re-runs `ldd` after bundling to fail loudly if
  any dep still doesn't resolve. Discovery is dynamic (parses ldd's
  "not found" lines) so the bundled set tracks ggml's actual NEEDED
  entries instead of a hard-coded list. NVIDIA's EULA explicitly
  permits redistribution of these specific runtime libs.

## [0.3.0-rc.3] - 2026-05-27

### Fixed

- **`release.yml` CUDA cublas install via Jimver still failed**
  (`.github/workflows/release.yml`). Jimver/cuda-toolkit always
  rewrites a sub-package name `X` to `cuda-X-<MAJOR>-<MINOR>` —
  there is no escape hatch to pass a name through verbatim. CUDA
  12.x's cublas packages are `libcublas-12-6` / `libcublas-dev-12-6`
  (no `cuda-` prefix), so both `cublas` (rc.1, → `cuda-cublas-12-6`)
  and `libcublas` (rc.2, → `cuda-libcublas-12-6`) resolved to
  nonexistent packages. The action now installs nvcc / cudart / cccl
  via Jimver and runs a separate `apt-get install libcublas-12-6
  libcublas-dev-12-6` step against the NVIDIA repo Jimver has
  already configured. ggml-cuda's MODULE build needs both the
  runtime `.so` and the dev headers (`cublas_v2.h`).

## [0.3.0-rc.2] - 2026-05-27

### Fixed

- **`release.yml` Linux CUDA install resolved nonexistent `cuda-cublas-12-6` package**
  (`.github/workflows/release.yml`). The Jimver/cuda-toolkit action
  expands a sub-package name without a `lib` prefix to
  `cuda-<name>-<MAJOR>-<MINOR>` (e.g. `cuda-nvcc-12-6`). CUDA 12.x
  ships cublas as `libcublas-12-6` / `libcublas-dev-12-6`, not
  `cuda-cublas-12-6`, so `apt-get install` failed at the toolkit
  install step and rc.1's Linux x86_64 build never reached `cargo
  build`. Sub-packages are now `["nvcc","cudart","cudart-dev",
  "libcublas","libcublas-dev","cccl"]` — the `lib`-prefixed forms
  are passed through verbatim by the action and resolve correctly.
- **`release.yml` Windows verify step couldn't load `llama.dll` from `backends/`**
  (`.github/workflows/release.yml`). `dl-backends` builds link the
  daemon against `llama.dll`, which `build.rs` stages into
  `backends/`. The Windows DLL loader resolves imported DLLs at
  process startup from the exe's own dir + PATH only — it does not
  search subdirectories. So `inferd-daemon.exe --help` aborted with
  exit 127 (`STATUS_DLL_NOT_FOUND`) before `main()` could run.
  `install.ps1` already flattens `backends\\*.dll` next to the exe at
  install time; the verify step now does the equivalent in-place so
  it mirrors the post-install layout. Linux + macOS unaffected
  (`$ORIGIN/backends` and `@loader_path/backends` are baked into the
  daemon's RPATH).

## [0.3.0-rc.1] - 2026-05-27

### Changed

- **Release tarball for Linux x86_64 now ships `libggml-cuda.so`**
  (`.github/workflows/release.yml`). The release workflow used to build
  with `--features inferd-daemon/dl-backends` only on Linux, which left
  `GGML_CUDA=OFF` in the cmake configure step — the resulting tarball
  had no CUDA MODULE so the v0.3 runtime accelerator probe always fell
  through to CPU on NVIDIA hosts. The workflow now installs the CUDA
  toolkit on the Ubuntu runner (Jimver/cuda-toolkit, pinned by SHA) and
  builds with `inferd-daemon/dl-backends,inferd-daemon/cuda`. A new
  staging assertion fails the release if `libggml-cuda.so` is missing
  from the produced `backends/` dir, and a parallel one for
  `libggml-metal.{so,dylib}` on `aarch64-apple-darwin`. Per-target
  features are now declared in the matrix entry's `features:` field
  rather than hard-coded in the build step.

### Fixed

- **macOS RPATH missing from daemon binary with `dl-backends`**
  (`crates/inferd-daemon/build.rs`, closes #26). `cargo:rustc-link-arg`
  emitted by a library crate's build script does NOT propagate to
  downstream binaries — only `rustc-link-search` and `rustc-link-lib`
  do. The daemon binary therefore had no `LC_RPATH` entries, and dyld
  reported "no LC_RPATH's found" at startup. Fixed by adding a
  `build.rs` to `inferd-daemon` that emits
  `-Wl,-rpath,@loader_path` and `-Wl,-rpath,@loader_path/backends`
  directly from the final binary's own link step. The daemon now starts
  from any directory without `DYLD_LIBRARY_PATH`.
- **`install-launchagent.sh` missed `.so` backend modules when
  flattening `backends/`** (`packaging/launchd/install-launchagent.sh`).
  `ggml_backend_load_all()` uses the `.so` extension on all Unix
  platforms including macOS; the Metal and CPU backend MODULEs are
  `libggml-metal.so` and `libggml-cpu.so`. The previous flatten step
  only copied `*.dylib`, so neither backend was ever loaded and the
  daemon fell through to "no registered backends → fail". Fixed by
  using `nullglob` and copying both `*.dylib` and `*.so` from
  `backends/` into the install dir.
- **`install-launchagent.sh` passed relative binary path to launchd**
  (`packaging/launchd/install-launchagent.sh`). launchd resolves a
  relative `Program` path relative to `/`, not the caller's cwd, so
  `./target/release/inferd-daemon` silently became
  `/target/release/inferd-daemon` and the service exited immediately
  (exit 78 / `ENOEXEC`). The script now resolves `$BIN` to an
  absolute path immediately after the existence check.
- **`dl-backends` first-boot config wrote `n_gpu_layers: 0`**
  (`crates/inferd-daemon/src/config_file.rs`). With dynamic backends the
  accelerator is picked at runtime; shipping `n_gpu_layers: 0` means
  Metal is selected but no layers are offloaded, matching CPU speed.
  The first-boot default for `dl-backends` builds is now `-1` (offload
  all layers), which is correct for any GPU path (llama.cpp clamps it to
  0 when no GPU device exists, so CPU-only hosts are unaffected).
  Operators who want explicit CPU mode can set `n_gpu_layers: 0` in
  `~/.inferd/config.json` after first boot.

### Added

- **Runtime accelerator probe**
  (`crates/inferd-engine/src/llamacpp/accelerator.rs`, gated on the
  new `dl-backends` feature). With `dl-backends` on, the daemon now
  calls `ggml_backend_load_all()` at first `LlamaCpp::new` and walks
  the ggml backend registry to pick the strongest backend the host
  actually has. The cascade is Metal → CUDA → ROCm → Vulkan → CPU;
  result is cached process-wide. Operators can force a specific
  pick with `INFERD_FORCE_BACKEND={cpu|metal|cuda|rocm|vulkan}` —
  useful for forcing CPU on a GPU host for benchmarking or
  sanity-checking that a particular accelerator's MODULE actually
  loads. The static-build path (no `dl-backends`) still honours the
  v0.2.x compile-time pick.
- **Device-detail surface on the admin `capabilities` frame**:
  `device_name` (e.g. `"NVIDIA GeForce RTX 4090"`, `"Apple M2 Pro"`)
  and `vram_total_bytes`, sourced from
  `ggml_backend_dev_name` / `ggml_backend_dev_memory` once
  `ggml_backend_load_all` has run. `inferdctl doctor` renders these
  on a new `device:` line when present. Backwards-additive on the
  admin wire — fields are omitted when null, and older subscribers
  ignore unknown keys per `docs/protocol-v1.md`. CPU path and
  cloud adapters keep both fields `None`.
- **Backend libraries staged into `target/<profile>/backends/`**
  (`crates/inferd-engine/build.rs`, closes #24). On the
  `dl-backends` build path, build.rs now copies every shared +
  MODULE library produced by cmake (`libllama`, `libggml`,
  `libggml-base`, every `ggml-cpu-<variant>`, plus whichever
  accelerator MODULEs were enabled — `ggml-metal`, `ggml-cuda`,
  `ggml-vulkan`, `ggml-hip`) into a stable
  `<workspace target>/<profile>/backends/` path. Releases (Phase
  5b) and platform install scripts (Phase 5d) need a predictable
  staging location; cmake-rs's hash-suffixed `OUT_DIR` is not it.
  Emits `INFERD_BACKENDS_DIR` as a `cargo:rustc-env` for downstream
  binaries that want to find the staged dir without re-deriving the
  same path. Static-build path is untouched (no shared artefacts to
  stage).
- **Release tarballs ship a `backends/` subdir**
  (`.github/workflows/release.yml`, closes #25). The release
  workflow now builds the daemon with `inferd-daemon/dl-backends`
  (was `inferd-daemon/llamacpp`) and bundles
  `target/<target>/release/backends/` next to the daemon binary in
  every platform tarball. Adds a verification step that fails the
  release loudly if the `backends/` dir is missing or near-empty —
  the silent-mock-tarball failure mode (v0.1.1, v0.1.4) but at the
  dl-backends layer. End users extract one tarball, run
  `./inferd-daemon`, and libllama dlopen's the right ggml backend
  from the tarball's own `backends/` dir via `$ORIGIN` /
  `@loader_path` RPATH — no system-wide install required.
- **CI matrix exercises the `dl-backends` path**
  (`.github/workflows/ci.yml`, closes #131). New `dl-backends` job
  runs `cargo clippy + test` on Linux/macOS/Windows with
  `inferd-engine/dl-backends`, then asserts
  `target/debug/backends/` is populated by `stage_backends_dir`.
  Catches regressions where someone breaks the staging hook or the
  shared-build cmake config without exercising the static
  `llamacpp` job. The `v0.3-dev` branch is added to the push +
  pull-request triggers so v0.3 work runs on every commit.
- **Install scripts handle the `backends/` co-location requirement**
  (closes #27). `ggml_backend_load_all()` only searches the
  executable's own directory, not subdirs — so the libs need to
  live next to the daemon, not under `backends/`. Each platform
  install path now handles this:
  - `packaging/windows/install.ps1`: when `-SourceBinary` is given,
    copies `<source>\backends\*.dll` into `%LOCALAPPDATA%\inferd\`
    next to `inferd-daemon.exe`.
  - `packaging/launchd/install-launchagent.sh`: detects a
    `backends/` sibling of the binary; if `<bindir>/libllama.dylib`
    is missing, flattens `backends/*.dylib` into `<bindir>/`. Refuses
    to write to dirs the user doesn't own.
  - `packaging/systemd/inferd.service` (Linux): unchanged unit, but
    the packaging README now documents the
    `cp backends/* ~/.local/bin/` step alongside the binary copy.
  Daemon binary now also has `RPATH=$ORIGIN` (Linux) /
  `@loader_path` (macOS) baked in at link time
  (`crates/inferd-engine/build.rs`), so libllama+ggml-* load from
  the install dir without `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH`
  gymnastics.

### Fixed

- **Static `llamacpp` build path was broken on `v0.3-dev`**
  (`crates/inferd-engine/build.rs`). Phase 5a gated the
  `stage_backends_dir` helper with `#[cfg(feature = "dl-backends")]`
  to silence an unused-function lint, but the call site still ran
  the function unconditionally behind the runtime bool
  `if dl_backends { stage_backends_dir(…) }` — `cfg!()` doesn't
  prevent rustc from resolving the symbol. Result: the static CI
  job (`cargo clippy --features inferd-engine/llamacpp`) failed
  with `cannot find function stage_backends_dir in this scope` on
  Linux + macOS. Rewrote both the staging call and the matching
  `cargo:rustc-link-arg=-Wl,-rpath,…` lines to live inside one
  `#[cfg(feature = "dl-backends")] { … }` block so rustc only sees
  them when the feature is on.

### Changed

- **Workspace bumped to `0.3.0-dev`.** v0.3 lands runtime accelerator
  detection per [ADR 0019](docs/adr/0019-runtime-accelerator-detection-via-ggml-backend-dl.md):
  Metal / CUDA / ROCm / Vulkan / CPU cascade picked at boot via
  llama.cpp's `GGML_BACKEND_DL` dynamic-loader path. v0.2.x ships
  CPU + platform-BLAS only; operators on GPU hardware were leaving an
  order of magnitude of throughput on the table. NPU paths
  (OpenVINO / ANE / DirectML-NPU / QNN) deliberately excluded — LLM
  decode lags CPU+SIMD on every shipping NPU in 2026.

## [0.2.4] - 2026-05-25

Single-bug release: any embed input over ~512 tokens (~2 KB
English) was crashing the daemon and triggering a systemd restart
loop. Real bug reported from cordon-filter integration with
EmbeddingGemma 300M (issue #20). v0.2.4 makes the embed pathway
robust: oversized inputs are rejected with a structured error and
the daemon stays alive; inputs that fit the configured embed
context (default 2048) flow through without hitting the libllama
encoder assert.

### Fixed

- **Embed: oversized inputs no longer abort the daemon**
  (`crates/inferd-engine/src/llamacpp/backend.rs`, closes #20).
  libllama's encoder asserts `n_ubatch >= n_tokens` and triggers
  `SIGABRT` (taking the whole daemon down) when an embed input
  tokenises beyond `n_ubatch`. The default `n_ubatch=512` meant a
  single ~2 KB English string was enough to crash inferd under
  modest batched embed load (e.g. cordon-filter sending 32 inputs
  per frame). Two-part fix: (1) the embed context now sets
  `n_batch = n_ubatch = embed_n_ctx`, so any input that fits in
  the configured embed context window also fits in one ubatch
  (raises the practical ceiling from ~512 to 2048 tokens with
  EmbeddingGemma 300M defaults); (2) `run_embed` now tokenises
  each input first and returns
  `EmbedError::InvalidRequest("input exceeds embed context: T
  tokens > n_ubatch N")` for anything still too large — mapped on
  the wire to `code: invalid_request` so the connection stays
  open and the caller sees a structured per-input error instead
  of a closed socket.

## [0.2.3] - 2026-05-23

Linux post-v0.2.2 follow-up: the v0.2.2 systemd `--user` unit set
`ProtectHome=read-only`, which blocked the daemon's first-boot
auto-pull path on Linux (the GGUF blobs and the default-config
write both live under `$HOME`). Plus a small DX fix that closes the
last of the v0.2.1 validation findings.

### Added

- **`--llamacpp-embed` / `--llamacpp-embed-pooling` /
  `--llamacpp-embed-n-ctx` CLI flags**
  (`crates/inferd-daemon/src/config.rs`,
  `crates/inferd-daemon/src/main.rs`). Closes #16. The legacy
  single-model config shape (`{ "model": {...} }`) and dev-mode
  (no config file) had no path to enable embed: `resolved_backends()`
  hard-coded `embed: false` when promoting the legacy shape, and the
  CLI-only llamacpp builder did the same. The new flags mirror the
  existing `--n-ctx` / `--n-gpu-layers` pattern: when set, they flow
  into both the legacy promotion path (overriding the hard-coded
  `embed: false`) and the dev-mode path. Multi-backend configs
  (`backends:`) keep full per-entry control — the CLI override only
  fires when the config used the legacy `model:` shape.

### Fixed

- **systemd `--user` unit: carve out CAS store + config dir under
  `ProtectHome=read-only`** (`packaging/systemd/inferd.service`,
  closes #18, PR #19). The v0.2.2 unit set
  `ProtectHome=read-only`, which blocked the daemon's first-boot
  auto-pull path: the CAS store under `~/.local/share/models/`
  (and the default config write to `~/.inferd/config.json`) sit
  inside `$HOME`, and read-only home blocks both. The directive
  defeated the v0.2.2 "install = work" contract on Linux. Replaces
  the read-only lock with a `ReadWritePaths=` carve-out for the
  two paths the daemon legitimately writes — keeps `ProtectHome=`
  blast-radius reduction across the rest of `$HOME` (ssh keys,
  browser data, shell history) where it actually matters. Also
  corrects an incorrect comment: `ReadWritePaths=` is honoured
  under `ProtectHome=` alone on systemd >= 232; it does **not**
  require `ProtectSystem=strict`.

## [0.2.2] - 2026-05-23

The "install = work" release. v0.2.0 / v0.2.1 shipped binaries that
required hand-editing `~/.inferd/config.json`, running `inferdctl pull`
before first boot, or passing `--backend llamacpp` on the command line
to actually do real inference. v0.2.2 makes a fresh install
(installer → real generate **and** real embed work) the contract:
no mock, no manual config, no pull-first preconditions, no flag
dance. Validated end-to-end on Linux (WSL), macOS Apple Silicon, and
Windows 11 before tag.

### Added

- **First-boot default multi-backend config**
  (`crates/inferd-daemon/src/config.rs`,
  `crates/inferd-daemon/src/main.rs`). When the daemon starts and no
  config file exists, it atomically writes a pinned default at
  `~/.inferd/config.json` declaring two real llamacpp backends:
  `gemma-4-e4b` (generate, ~5.1 GB) and `embeddinggemma-300m`
  (embed, ~313 MiB), both with `auto_pull = true` and pinned
  SHA-256s. Operators no longer have to author a config by hand to
  get inference; the next daemon boot sees the file, fetches both
  blobs into the CAS store, and brings up both backends. Closes
  the install-time DX gap that made every prior v0.2.x cut feel
  like a developer build.

- **Capability-aware embed routing**
  (`crates/inferd-daemon/src/router.rs`,
  `crates/inferd-daemon/src/lifecycle_embed.rs`). The router now
  exposes `dispatch_embed()` which filters registered backends by
  `capabilities().embed` so embed requests skip generate-only
  backends entirely, instead of the previous "first registered
  backend wins, embed errors out at the backend layer" path. Two
  new tests use a `GenerateOnly` mock wrapper that overrides
  `capabilities().embed = false` to exercise the dispatch filter.

- **`packaging/windows/uninstall.ps1`** (new). Removes the Startup
  shortcut, stops the running daemon, and (with `-Purge`) deletes
  the staged binary, lock, logs, and `~/.inferd/config.json`. Models
  in the CAS store at `%LOCALAPPDATA%\models\` are intentionally
  left intact — re-pulling multi-GB blobs is slow and the operator
  can wipe the directory themselves.

- **`packaging/windows/cleanup-legacy-service.ps1`** (new) — one-shot
  cleanup helper for operators upgrading from a v0.2.1 install whose
  SCM service was registered with the hardened SDDL that strips
  `DELETE`/`WRITE_DAC`/`WRITE_OWNER` from Administrators. The script
  self-elevates via UAC, takes ownership of the
  `HKLM:\SYSTEM\CurrentControlSet\Services\inferd-daemon` registry
  key, grants Administrators `FullControl`, deletes the key, and
  prints the reboot-to-flush-SCM-cache instruction. Required because
  the bad SDDL blocks `sc.exe delete` even when run elevated, so
  there is no in-band way for an operator to remove the legacy
  registration without registry-level surgery. The new `install.ps1`
  surfaces a warning pointing at this script when it detects a
  legacy registration during install.

### Changed

- **`--backend` is now `Option<BackendKind>`**
  (`crates/inferd-daemon/src/main.rs`). Previously the clap derive
  defaulted to `BackendKind::Mock` when the flag was unset, which
  silently short-circuited config-file backend loading and shipped
  a mock daemon to operators who thought they had llamacpp wired up.
  Now: omitted flag → defer to `~/.inferd/config.json`; explicit
  `--backend mock` → mock (useful in test rigs where an unrelated
  config file is on disk); explicit non-mock → CLI-flag-only path,
  config `backends:` ignored. This is the change that lets the
  default-config + Startup-shortcut combo actually serve real
  inference on first boot.

- **Drop `--backend mock` and pull-first precondition from all 3
  install manifests** (`packaging/launchd/io.inferd.daemon.plist`,
  `packaging/launchd/install-launchagent.sh`,
  `packaging/systemd/inferd.service`,
  `packaging/windows/install.ps1`). The macOS LaunchAgent template
  previously had `__BACKEND__` / `__MODEL_PATH__` placeholders the
  install script never substituted, so the daemon defaulted to the
  mock backend even after `inferdctl pull` (#9 root cause carried
  into v0.2). The macOS install script also required `inferdctl
  pull` to have run first as a precondition, breaking the install =
  work contract. Both gone in v0.2.2 — installer just runs the
  daemon with the default config.

- **`--embed` enabled by default in all 3 install manifests**
  (`packaging/launchd/io.inferd.daemon.plist`,
  `packaging/systemd/inferd.service`,
  `packaging/windows/install.ps1`). The embed socket bind is gated
  on `--embed` per ADR 0017; without it a fresh install never binds
  the embed pipe and operators see "no embed-capable backend
  available" on first embed call. Each manifest now passes `--embed`
  + an explicit `--embed-addr` matching the platform default.

- **Windows install: drop SCM service, use Startup-folder shortcut**
  (`packaging/windows/install.ps1`, new `packaging/windows/uninstall.ps1`,
  `packaging/README.md`, `.github/workflows/release.yml`). The previous
  installer registered an SCM service via `sc.exe create`. That path
  can't work as written: the daemon binary is a foreground console
  app with no `StartServiceCtrlDispatcher` registration, so SCM
  killed it after the 30-second start timeout (Event 7000/7009).
  Beyond the structural mismatch, the install required elevation,
  the staged binary lived in `%LOCALAPPDATA%` (NetworkService can't
  read it without an `icacls` grant), and the SDDL stripped
  `DELETE`/`WRITE_DAC`/`WRITE_OWNER` from Administrators — locking
  operators out of `sc.exe delete inferd-daemon` even when elevated.
  The new installer creates a `.lnk` in `shell:startup` pointing at
  `%LOCALAPPDATA%\inferd\inferd-daemon.exe`, so the daemon launches
  on every login as the current user. **No elevation required.**
  Matches the macOS LaunchAgent and Linux systemd --user posture:
  per-user, no SCM, stops at logout. The new `uninstall.ps1` removes
  the shortcut, stops the running daemon, and (with `-Purge`)
  deletes the staged binary, lock, logs, and config. The hardening
  table in `packaging/README.md` is updated to drop the `sc.exe sdset`
  service-ACL row that no longer applies.

### Fixed

- **`inferdctl status` tolerates v0.2 capabilities frame; surfaces
  manifest path on parse error** (`crates/inferd/src/main.rs`). The
  v0.2.0 daemon began emitting a `capabilities` admin frame before
  the existing `status` frame; the v0.1 CLI parser treated unknown
  frames as fatal and exited non-zero. Now the CLI ignores
  forward-compatible frame types and prints which manifest path
  failed parsing when it does encounter a malformed frame, instead
  of a bare serde error.

- **`ureq` TLS: load OS trust store via `tls` + `native-certs`** (`crates/inferd-daemon/Cargo.toml`).
  The previous `features = ["tls"]` used rustls with the bundled `webpki-roots` only, which
  does not see system-installed CA certificates (corporate proxies with TLS interception, internal
  CAs). First attempt swapped to `native-tls` + `native-certs`, but ureq 2.x's `native-tls` feature
  requires explicit `.tls_connector(Arc<TlsConnector>)` glue on `AgentBuilder` that we don't have —
  Windows builds errored at fetch time with `Unknown Scheme: cannot make HTTPS request because no
  TLS backend is configured`. Final shape pairs ureq's `tls` (rustls, auto-initialized) with
  `native-certs` (rustls-native-certs feeds the OS trust roots into rustls). Works across all three
  platforms with no glue code, same observable behaviour as `curl` for system-trusted CAs.

## [0.2.1] - 2026-05-22

### Fixed

- **macOS LaunchAgent install fixes carried forward from v0.1.13/v0.1.14
  on `main`** (cherry-picked from commits `486b392` and `2a6d19e`). The
  `v0.2-dev` branch never picked up two real launchd bugs that landed
  on `main` while v0.2 work was in flight:
  (1) `launchctl bootstrap` on a previously bootout-ed agent returned
  `EX_IO (5)` because launchd marks the agent disabled after bootout —
  fix by calling `launchctl enable` before `bootstrap` in
  `packaging/launchd/install-launchagent.sh`;
  (2) `io.inferd.daemon.plist` had `__BACKEND__` / `__MODEL_PATH__`
  placeholders that the install script never substituted, so the daemon
  defaulted to the mock backend even after `inferdctl pull` — fix by
  threading the values through the install script and failing loudly if
  no model is provided. Without these, the v0.2.x tarball produced a
  broken macOS install path. Closes #9.

### Changed

- **CLI binary renamed `inferd` → `inferdctl`** (ADR 0018,
  supersedes ADR 0014's name choice; ADR 0014's invariants are
  preserved). The standalone `inferd` crate name on crates.io is
  squatted, blocking `cargo publish` of the CLI; rather than
  pursue an ownership dispute we land on a `*ctl`-suffixed name
  that both publishes cleanly and disambiguates the CLI from
  `inferd-daemon` for operators (cf. `systemctl`, `kubectl`).
  The architectural posture is unchanged — the CLI is still a
  peer reference-middleware client of every other consumer, with
  no private daemon API. Touched: `crates/inferd/Cargo.toml`
  (package + bin name), `crates/inferd/src/main.rs` (clap
  `name = "inferdctl"` + doc-comments),
  `.github/workflows/release.yml` (build + staging),
  `INTEGRATING.md`, daemon source comments referencing the CLI.
  The directory `crates/inferd/` did not move — only the
  published crate name and binary basename. **Migration:**
  shell scripts referencing `inferd status` / `inferd watch` /
  `inferd pull` / `inferd doctor` need to be updated to
  `inferdctl <subcommand>`. Closes #100.

### Added

- **Release pipeline hardening** (`.github/workflows/release.yml`,
  `docs/RELEASING.md`). The v0.2.0 cut shipped with no platform tarballs
  attached because the macOS build broke and the `Sign + publish` job
  (`needs: [build, sbom]`) was skipped — exposing several latent
  fragility points in the release pipeline. Five-part hardening so v0.2.1
  can ship cleanly:
  - **SHA256 sidecars.** Each platform job now generates a `*.sha256`
    file next to its archive and uploads it alongside. Universal
    "did this download corrupt" check; verify with `shasum -a 256 -c
    <archive>.sha256`. Cosign bundles still ship for provenance.
  - **Pinned third-party Actions.** Every `uses:` reference now pins
    to a commit SHA with the upstream tag name as a comment
    (`actions/checkout@de0fac2e... # v6`). Mitigates tag-rewrite
    attacks on release pipelines, consistent with the supply-chain
    posture in `~/.claude/security-scanners.md`.
  - **CHANGELOG section as release body.** The `publish` job now
    extracts the matching `## [X.Y.Z]` section from `CHANGELOG.md`
    and uses it as the release body. Replaces the previous
    auto-generated PR list. Falls back to auto-generation if the
    section is missing.
  - **Asset-completeness sanity check.** New step in `publish`
    asserts: 4 archives, 4 sha256 sidecars, 4 cosign bundles, ≥1
    SBOM, with archive/sha/bundle counts matching. Fails the
    workflow loudly before creating a half-populated release page.
  - **`docs/RELEASING.md`.** Human-readable release runbook: what a
    release ships, how to cut one, what the trajectory looks like in
    Actions, what to do when something goes wrong (mid-workflow
    failure, missing assets, wrong release body, cosign signing
    issues).

### Fixed

- **macOS aarch64 release build** (`crates/inferd-engine/cpp/CMakeLists.txt`):
  the Phase 3A CMake wrapper used `add_subdirectory(... EXCLUDE_FROM_ALL)`,
  which skips any upstream target not transitively depended on by an
  `ALL`-attached target. `ggml-blas` was never in that transitive set, so it
  was never built or installed on macOS — producing "cannot find native static
  library `ggml-blas`" at link time. Fixed by:
  (1) setting `GGML_BLAS=ON` / `GGML_BLAS_VENDOR=Apple` before
  `add_subdirectory` so upstream's CMake defines the target at all;
  (2) adding `add_dependencies(ggml ggml-blas)` after `add_subdirectory`
  (guarded by `if(TARGET ggml-blas)`) so the EXCLUDE_FROM_ALL is
  overridden by the explicit dependency chain;
  (3) adding a matching `install(TARGETS ggml-blas ...)` so the archive
  lands in `${CMAKE_INSTALL_PREFIX}/lib` where `build.rs`'s link-search
  finds it. Validated locally on arm64 macOS: `libggml-blas.a` now
  appears in `OUT_DIR/lib/` and the daemon binary links cleanly. Closes #12.

## [0.2.0] - 2026-05-21

### Fixed

- **Phase 6B-7 part 8: `LlamaCpp::embed` reports the actual model
  identifier on response frames.** Previously the adapter hard-coded
  `model: "llamacpp"` on every `embeddings` frame — a duplicate of
  `Backend::name()` and useless to operators trying to confirm which
  GGUF served their request. The adapter now reads GGUF `general.name`
  metadata via `llama_model_meta_val_str` at construction time and
  caches the result on the adapter; if the key is absent it falls
  back to the path file stem (e.g. `embeddinggemma-300m-Q8_0` from
  `embeddinggemma-300m-Q8_0.gguf`), and as a last resort the constant
  `"llamacpp"` for paths with no valid Unicode stem. Diagnostic-only
  per ADR 0007 — apps must not branch on this — but accuracy here
  matters for log correlation and `inferd doctor` parity.

### Added

- **Phase 6B-7 part 9: real-model embed integration test.** New
  `crates/inferd-engine/tests/embed_llamacpp.rs` (gated behind
  `llamacpp-integration`, skips when `INFERD_TEST_EMBED_MODEL_PATH`
  is unset) drives the FFI path against a real EmbeddingGemma 300M
  GGUF: capability advertisement, default-dim-768 unit-norm output,
  in-order batching with cosine-distinct vectors, MRL truncation to
  256 with re-normalisation, rejection of dimensions above `n_embd`,
  task-prefix variation across `RetrievalQuery` / `RetrievalDocument`
  / unprefixed, and a smoke pass through all eight EmbeddingGemma
  task variants. No mocks anywhere on this path — same Tier-3 shape
  as `tests/llamacpp.rs` and `tests/llamacpp_multimodal.rs`.

- **Phase 6B-7 part 7: config-file embed fields on `LlamacppEntry`.**
  `LlamacppEntry` (multi-backend `backends:` shape) gains `embed:
  bool` (default `false`), `embed_pooling: Option<i32>` (default
  `None`, treated as `LLAMA_POOLING_TYPE_MEAN` by the adapter),
  and `embed_n_ctx: u32` (default `2048` — EmbeddingGemma 300M's
  window). The daemon's `build_llamacpp_from_entry` plumbs these
  three fields straight through to `LlamaCppConfig`, so an
  operator who declares `embed: true` on a backend gets a
  capability-advertising backend without further wiring. Legacy
  single-`model:` configs predate ADR 0017 and stay generation-
  only — the legacy promotion path explicitly sets `embed: false`
  with the embed-context defaults so the field shape stays
  consistent. Three new tests cover (1) default-off behaviour
  when the operator omits the fields, (2) round-trip when all
  three are set, and (3) the legacy promotion path still flips
  embed off. Workspace clippy + tests stay green; this closes
  out Phase 6B-7 (#97), unblocking #87 (v0.2.0 tag) for explicit
  human go-ahead.

- **Phase 6B-7 part 6: INTEGRATING.md embed section.**
  Added an "Embeddings (v0.2)" section with the third-socket
  endpoint table, capability-discovery / `inferd doctor` snippet,
  config-file shape (`llamacpp` entry with `embed: true`,
  `embed_pooling`, `embed_n_ctx`), Rust example, supported
  request fields (`input` / `dimensions` / `task` with the
  EmbeddingGemma task taxonomy), and the embed-specific error
  contract. Bedrock-invoke also added to the "Backends in v0.2"
  list (was missing). Versioning section now mentions the embed
  wire as separate-socket-frozen-once-shipped per ADR 0017.

- **Phase 6B-7 part 5: `inferd-client` embed surface.** New
  `EmbedClient` (sibling to `Client` / `ClientV2`) ships
  `dial_tcp` / `dial_uds` (Unix) / `dial_pipe` (Windows) and a
  single `embed(req)` method that round-trips one terminal
  `EmbedResponse` per `EmbedRequest`. The connection stays open
  for the next call — long-lived semantics match v1 / v2.
  Default endpoint resolution (`default_embed_addr`) mirrors the
  daemon's `endpoint::default_embed_addr` (XDG → `~/.inferd/run`
  → `/tmp` on Linux, `${TMPDIR}/inferd` on macOS, named pipe on
  Windows). `dial_and_wait_ready` is generic over the client type
  so the existing F-13 retry shape serves embed clients without
  duplication. Embed wire types are re-exported (`EmbedRequest`,
  `EmbedResponse`, `EmbedTask`, `EmbedErrorCode`, `EmbedUsage`,
  `EmbedResolved`) so consumers don't need a separate
  `inferd-proto` dep. Four new unit tests cover success-frame and
  error-frame round-trips, EOF handling, and connection reuse
  across multiple requests. INTEGRATING.md update lands in part 6.

- **Phase 6B-7 part 4: daemon embed socket lifecycle.** Daemon now
  binds a dedicated third inference socket (`/inferd-infer-embed`
  UDS / `\\.\pipe\inferd-infer-embed` named pipe / `--embed-tcp`
  loopback) per ADR 0017 when `--embed` is requested *and* at
  least one registered backend advertises `capabilities().embed`.
  The new `lifecycle_embed` module mirrors `lifecycle_v2` but
  short-circuits to a single terminal frame per request
  (`embeddings` or `error`) — no streaming. Admission, F-7
  peer-cred (UDS / pipe), and F-8 first-frame TCP API-key gates
  are reused from the existing accept context, and embed dispatch
  shares the one warm-model admission slot with v1+v2 (one slot
  is one slot per ADR 0012). Capability advertisement
  (`StatusEvent::Capabilities { embed, .. }`) flows through to
  the admin socket, the `inferd-client` `AdminEvent`, and the
  `inferd doctor` / `inferd watch` surfaces so operators can see
  whether the warm backend can serve embeddings. CLI gains
  `--embed`, `--embed-addr`, `--embed-tcp` (with the same
  conflicts-with shape as v2) and `listen.tcp_embed` joins the
  config-file shape. CLI flag without a capable backend warns
  and skips binding rather than failing — keeps the v0.2 cloud
  adapters (which legitimately don't embed yet) running clean.
  Workspace clippy + tests stay green; client surface lands in
  part 5.

- **Phase 6B-7 part 3: llamacpp embed adapter.** `LlamaCppConfig`
  grows `embed: bool`, `embed_pooling: Option<i32>` (defaults to
  `LLAMA_POOLING_TYPE_MEAN` — what EmbeddingGemma expects), and
  `embed_n_ctx: u32` (defaults to 2048, the EmbeddingGemma 300M
  context). When `embed = true` the adapter allocates a dedicated
  second `llama_context` configured with `embeddings = true` so
  `Backend::embed` doesn't have to toggle `llama_set_embeddings` on
  the live generation context (which would race active generations).
  `capabilities().embed` flips `true` accordingly. The new
  `LlamaCpp::embed` impl applies EmbeddingGemma's task-prefix
  convention before tokenisation (eight `EmbedTask` variants mapped
  to the documented prefixes; `None` and `Other` pass through
  unchanged), tokenises each input under the lock guard, runs
  `llama_encode`, reads the pooled per-sequence vector via
  `llama_get_embeddings_seq`, applies Matryoshka truncation when
  `dimensions` is set (rejects values larger than the model's
  `n_embd`), L2-renormalises the truncated vector, and returns one
  `Vec<f32>` per input. The FFI work runs on `spawn_blocking` so it
  doesn't stall the tokio runtime. `bedrock_invoke` and
  `openai_compat` adapters keep `embed: false` per ADR 0017's
  v0.2.0 scope. Workspace clippy + tests stay green; daemon
  socket binding lands in part 4.

- **Phase 6B-7 part 2: `Backend::embed` trait method + Mock impl.**
  Engine crate gains `EmbedResult` (one vector per input + dimensions
  + model name + `EmbedUsage`) and `EmbedError` (its own taxonomy:
  `NotReady`, `Unsupported`, `InvalidRequest`, `Unavailable`,
  `Internal` — distinct from `GenerateError` because the embed
  surface has no streaming, no mid-stream concept, and adds the
  not-an-embed-backend case). The `Backend` trait grows a default
  `embed()` returning `EmbedError::Unsupported` so existing adapters
  (`bedrock_invoke`, `openai_compat`) compile unchanged; opt-in is
  via `capabilities().embed = true`. The Mock backend opts in and
  returns deterministic vectors derived from input length so daemon
  embed-socket dispatch can be exercised end-to-end without a real
  engine. Five new mock-backend tests cover the cap advertisement,
  vector-shape determinism, requested-dimensions honoring,
  pre-stream-error mapping, and not-ready short-circuit. Workspace
  clippy + tests stay green; `llamacpp` adapter wiring lands in part
  3.

- **Phase 6B-7 part 1: embed wire types in `inferd-proto`.** New
  `embed` module (sibling to `v2`) ships `EmbedRequest` / `EmbedResolved`
  / `EmbedTask` / `EmbedResponse` / `EmbedErrorCode` / `EmbedUsage`,
  matching ADR 0017's locked envelope. Single-frame request: `id`
  (correlation), `input` (non-empty array of non-empty strings),
  optional `dimensions` (MRL truncation length, validated at the
  backend layer), optional `task` (task-prefix hint with eight
  EmbeddingGemma-shaped variants plus a forward-compatible `Other`
  catchall that `resolve()` rejects). Single-frame response: either
  `Embeddings { embeddings, dimensions, model, usage, backend }` or
  `Error { code, message }` — no streaming, since an embedding is a
  complete vector. Error taxonomy matches v1's plus `embed_unsupported`
  for the belt-and-braces case where the embed socket somehow gets
  bound on a generation-only daemon. Nine new tests cover empty-input
  rejection, empty-inner-string rejection, full-JSON round-trip,
  unknown-task forward-compat, and serializer field elision.

- **ADR 0017: embeddings on a third socket.** Locks the v0.2.0 wire shape
  for embedding requests before code lands. Embeddings ship on a
  dedicated NDJSON-over-IPC socket (`infer.embed.sock` /
  `\\.\pipe\inferd-infer-embed`) — same framing as v1 / v2, separate
  path. HTTP `/v1/embeddings` stays an ecosystem-extension job per
  ADR 0006. ADR 0012's "one warm model per process" rule stands:
  operators who want both generation and embeddings run two inferd
  processes. The capability frame on the admin socket gains an
  `embed: bool` field; the daemon binds the embed socket only when
  the active backend reports `supports().embed == true`. v0.2.0
  scope is llamacpp + EmbeddingGemma 300M only — `openai-compat`
  `/v1/embeddings` and Bedrock Titan Embed are explicitly deferred
  to v0.2.1+.

- **Phase 6B-5 part 2: bedrock-invoke wired into the daemon binary.**
  New daemon-side `bedrock` cargo feature; `--backend bedrock-invoke`
  CLI flag plus `--bedrock-region`, `--bedrock-model-id`,
  `--bedrock-bearer-token` (env: `AWS_BEARER_TOKEN_BEDROCK`),
  `--bedrock-endpoint`, and `--bedrock-timeout-secs` for the
  CLI-only path; matching `kind: "bedrock-invoke"` config-file
  entry with `region`, `model_id`, optional `bearer_token_env`
  (env-var-by-name shape, mirroring openai-compat's
  `api_key_env` so secrets stay out of the file), optional
  `endpoint`, and `timeout_secs`. Auth resolves bearer-first
  (CLI flag → named env var), then the standard
  `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / optional
  `AWS_SESSION_TOKEN` chain via SigV4. Operators with no auth
  configured get a clear startup error pointing at both options.
  Five new tests cover the CLI-shape round-trip + defaults and
  the config-file BedrockInvokeEntry round-trip + validation
  (empty region / empty model_id rejected).
- **Phase 6B-5 part 1: bedrock-invoke backend adapter (engine crate).**
  New `bedrock_invoke` module behind the `bedrock` cargo feature
  ships the AWS Bedrock-runtime
  `InvokeModelWithResponseStream` plumbing: a hand-rolled SigV4 signer
  (HMAC-SHA-256 chain, ~150 lines — avoids pulling in the AWS SDK
  ecosystem), an AWS event-stream binary frame decoder
  (`application/vnd.amazon.eventstream`, 1 MB safety cap, CRCs trusted
  to TLS), an Anthropic-on-Bedrock body mapper for the locked
  `anthropic_version: "bedrock-2023-05-31"` shape, and a
  `StreamAccumulator` that absorbs the inner Anthropic SSE-shaped
  events (`message_start` → `content_block_delta`* →
  `content_block_stop` → `message_delta` → `message_stop`) and emits
  `TokenEventV2`. Two auth modes, in order: bearer token
  (`AWS_BEARER_TOKEN_BEDROCK`, sent as `Authorization: Bearer`, skips
  SigV4) and the standard AWS access-key chain. v0.2.0 multimodal
  gate is single-sided — image/audio/video content blocks are
  rejected at request build time; tool-use is supported in both
  directions. 35 unit tests cover the body mapper round-trips, the
  event-stream decoder against partial feeds and exception frames,
  the SigV4 signer's determinism + session-token handling, and the
  adapter's URL/host/header construction. The adapter is built but
  not yet wired into the daemon binary — that lands in part 2
  (`BackendKind::BedrockInvoke` + `kind: "bedrock-invoke"` config-
  file entry).
- **Phase 6B-4: TCP transport as config opt-in (default off).** Closes
  the v0.2.0-tagging gap that enabling TCP required a CLI flag, which
  blocked operators who run inferd under a system service manager
  (systemd, launchd, NSSM) where editing the unit file just to flip a
  transport is friction. Config-file schema grows an optional `listen`
  block with `tcp` (v1 inference socket bind address), `tcp_v2` (v2
  inference socket bind address — only honoured when `--v2` is also
  set), and `api_key_env` (env-var **name** carrying the pre-shared
  TCP API key, mirroring the `openai-compat` env-var-by-name shape so
  secrets stay out of the file). Default behaviour is unchanged: when
  the operator passes `--tcp`/`--uds`/`--pipe`, the CLI flag wins and
  the config block is ignored with a one-line info log; when no CLI
  transport is set, `listen.tcp` is the fallback. Errors with a clear
  punch-list when neither source supplies a transport (`pass
  --tcp/--uds/--pipe on the CLI, or set listen.tcp in
  ~/.inferd/config.json`). API-key resolution chain: CLI `--api-key`
  → `config.listen.api_key_env` → `INFERD_API_KEY` env (already wired
  via clap's `env=`). The "no api-key configured but TCP listener
  bound" warning (THREAT_MODEL F-8) now fires for both CLI-driven and
  config-driven TCP. Restart-time only — no config watcher; operators
  changing `listen:` must restart the service. Targets WSL ↔ Windows
  host and podman cross-VM boundary scenarios where Unix sockets and
  named pipes don't cross the VM line cleanly. Three new
  `config_file::tests` cover the listen-absent default, the full
  `tcp` + `tcp_v2` + `api_key_env` round-trip, and empty-string
  rejection.
- **Phase 6B-3: multi-backend config shape.** Closes the v0.2.0-tagging
  gap that `~/.inferd/config.json` only carried a single `model:` entry
  even though the engine, router, and circuit breaker were already
  designed for the multi-backend world. The schema grows a new
  optional `backends:` array of `kind:`-tagged entries — `kind:
  "llamacpp"` (with its own `model:` block, `n_ctx`, `n_gpu_layers`)
  and `kind: "openai-compat"` (with `base_url`, `model`, optional
  `api_key_env`, `timeout_secs`) — that the router walks in order per
  ADR 0007. The legacy top-level `model:` field stays optional and
  keeps working: when set without `backends:`, `resolved_backends()`
  auto-promotes it to a one-element `[{kind: "llamacpp", ...}]` list,
  so v0.1.x configs land unmodified. Setting both is a parse-time
  validation error. API keys for `openai-compat` are referenced by
  env-var **name** via `api_key_env: "<NAME>"` — never embedded
  literally in the file — and the daemon resolves through
  `api_key_env` → `INFERD_OPENAI_API_KEY` → `OPENAI_API_KEY` → empty
  (skipping the `Authorization` header) so secrets stay in env, not
  on disk. Daemon `build_backends` now returns
  `Vec<Arc<dyn Backend>>` and feeds the router directly; one
  `Capabilities` admin frame fires per backend so subscribers see
  the full router shape. The `inferd doctor` and `inferd pull`
  subcommands walk every llamacpp entry (each with its own blob /
  manifest) and skip cloud entries. The `kind:` field is an
  open-ended tagged union so future variants (`bedrock-invoke`, …)
  slot in additively without a config break. Twelve new
  `config_file::tests` cover the multi-backend happy path,
  mutual-exclusion validation, duplicate-name rejection, scheme
  validation (http/https only), unknown-kind parse error, and the
  legacy-model auto-promotion path.
- **Phase 6B-2: `BackendKind::OpenaiCompat` wired into the daemon
  binary.** Closes the v0.2.0-tagging gap that the engine shipped an
  OpenAI-compat adapter behind `--features openai` but the daemon
  binary couldn't select it. New `BackendKind::OpenaiCompat` variant
  (gated on the daemon's `openai` cargo feature, which feeds through
  to `inferd-engine/openai`) registers as `--backend openai-compat`
  via clap. Four new CLI flags carry the adapter's config:
  `--openai-base-url` (env `INFERD_OPENAI_BASE_URL`),
  `--openai-api-key` (env `INFERD_OPENAI_API_KEY`, `hide_env_values`
  to keep the bearer out of `--help`), `--openai-model` (env
  `INFERD_OPENAI_MODEL`), and `--openai-timeout-secs` (env
  `INFERD_OPENAI_TIMEOUT_SECS`, default 300s). The API key
  resolution chain is `--openai-api-key` → `INFERD_OPENAI_API_KEY` →
  `OPENAI_API_KEY` (the de-facto env name most provider SDKs already
  use); pass an empty string to skip the `Authorization` header
  entirely for self-hosted endpoints (vLLM, LM Studio, LocalAI,
  llama.cpp's HTTP server). The `build_openai_compat` builder
  publishes a `LoadingModel{CheckingLocal}` status event tagged with
  `(openai-compat: <base_url> / <model>)` so the admin status feed
  surfaces *which* upstream the daemon is configured for.
- **Phase 6B-1: `inferd-client` v2 surface.** New `ClientV2` mirrors
  `Client`'s shape (`dial_tcp` / `dial_uds` / `dial_pipe`) but speaks
  the v2 wire types (`RequestV2` / `ResponseV2`) per ADR 0015.
  `generate(RequestV2)` returns a `FrameStreamV2` of `ResponseV2`
  frames, terminating on `Done` / `Error` exactly like v1. Defaults
  helper `default_v2_addr()` returns the same per-platform fallback
  chain the daemon binds (`${XDG_RUNTIME_DIR}/inferd/infer.v2.sock`
  on Linux, `${TMPDIR}/inferd/infer.v2.sock` on macOS,
  `\\.\pipe\inferd-infer-v2` on Windows). `dial_and_wait_ready` is
  now generic over the client type so the same retry helper serves
  both v1 and v2 — existing callers infer the client type
  unchanged. v2 wire types (`RequestV2`, `ResponseV2`,
  `ContentBlock`, `MessageV2`, `RoleV2`, `Attachment`, `Tool`,
  `ToolCallId`, `ToolUseInput`, `ResponseBlock`, `StopReasonV2`,
  `ErrorCodeV2`, `UsageV2`, `ResolvedV2`) re-exported at the crate
  root so consumers don't need a separate `inferd-proto` dep. Two
  unit tests cover the streams-frame-then-done happy path and the
  unexpected-EOF error path. Closes the v0.2.0-tagging gap that the
  shipped daemon spoke v2 but the published client could not.
- **Phase 6B: v0.2.0 release prep.** Workspace version bumped to
  `0.2.0`. All intra-workspace pinned deps (`=0.1.13`) bumped in
  lockstep to `=0.2.0` so each crate's published artefact resolves
  consistently. INTEGRATING.md migrated from "v0.2 preview" framing
  to v0.2 reality: the v2-wire section now documents the
  default-bound socket paths (`infer.v2.sock` / `\\.\pipe\inferd-
  infer-v2`), the raw-bytes attachment posture (ADR 0016 — the
  daemon does not link image / audio codecs; consumers decode), the
  tool-call lifecycle (`tool_use` content blocks in stream,
  `tool_result` blocks back), the in-place migration shape
  (`Message.content: String` → `Vec<ContentBlock>`), and the v0.2
  backend matrix (`llamacpp` default + feature-gated `openai`
  outbound HTTPS adapter). Versioning section updated to call out
  that `inferd-client = "0.1"` consumers keep working unmodified
  against the v1 socket of a v0.2 daemon.
- **CI: v2 + openai-compat matrix coverage** (Phase 6A). New
  `openai` job runs `cargo clippy --features inferd-engine/openai`
  and `cargo test -p inferd-engine --features openai` on the same
  three-OS matrix as the default suite, catching mapper / SSE
  drift before it ships. The existing `systemd-unit` smoke job now
  starts the daemon with `--v2` (sed-injected into the shipped
  `inferd.service` unit at install time, leaving the canonical
  v1-only file untouched), verifies `infer.v2.sock` exists with the
  spec-mandated `0660` mode, and round-trips a v2 NDJSON request
  (`messages[].content` typed blocks, `text` block) through the v2
  UDS to a `done` frame from the mock backend. Catches regressions
  in v2 socket binding, v1+v2 AcceptContext sharing, RequestV2
  resolve, and the router's V2-capable check.
- **inferd-daemon: real router policy** (Phase 5B, per ADR 0007).
  `crates/inferd-daemon/src/router.rs` rewritten from the v0.1 single-
  backend stub into a priority-ordered router with per-backend circuit
  breaker. Public surface: `Router`, `Dispatch { backend, name }`,
  `BreakerPolicy { failure_threshold, failure_window, cooldown }`,
  `RouterError { NoBackends, NoneAvailable }`. Defaults: 3 failures in
  60s opens the breaker; 30s cooldown; first dispatch after cooldown
  enters half-open and one outcome (success closes / failure re-opens)
  decides next state. `dispatch()` walks slots in priority order, skips
  not-ready and open-breaker slots, returns `NoneAvailable` if every
  registered backend is unfit. New name-keyed feedback methods
  `record_success(name)` / `record_failure(name)` are O(1) via a
  `name → index` map populated at construction (backends are static
  in v0.2; no admin add/remove API). Lifecycle wiring in both
  `lifecycle.rs` and `lifecycle_v2.rs`: pre-stream `GenerateError`
  paths distinguish `is_backend_failure` (NotReady / Unavailable /
  Internal trip the breaker; `InvalidRequest` does not — caller bug
  is not a backend health signal); terminal `Done` calls
  `record_success`; mid-stream silent termination calls
  `record_failure`. No retry, no failover (ADR 0007). 8 unit tests
  exercise empty-router rejection, ready-backend dispatch, unready
  fallthrough, priority ordering, threshold-trip, success-resets-
  count, post-cooldown half-open recovery, sliding-window pruning,
  and unknown-backend feedback no-op. Dev-dep `async-trait` added
  for the named-backend test harness.
- **inferd-engine: OpenAI-compat HTTP backend adapter** (Phase 5A,
  feature-gated behind `openai`). New `openai_compat` module
  implementing `Backend` against any upstream that speaks the
  OpenAI Chat Completions wire (OpenAI itself, vLLM, LM Studio,
  LocalAI, llama.cpp's `server`, OpenRouter, …). The narrow
  outbound-HTTPS carve-out lives behind the `Backend` trait per
  ADR 0006 §"cloud backends" — the daemon never *serves* HTTP.
  Public surface: `OpenAiCompat` (the adapter), `OpenAiCompatConfig`
  (`base_url`, `api_key`, `model`, `timeout`), `OpenAiCompatError`.
  Capabilities advertise `v2 + tools` only — multimodal stays off
  in v0.2 (raw-bytes attachment shape from ADR 0016 doesn't map
  cleanly to OpenAI's `image_url` data-URL form), and thinking
  stays off (no public reasoning channel on Chat Completions).
  v1 path is rejected with `Internal("openai-compat backend
  supports v2 only")`. Wire shape:
    - Request mapping (`mapper::request_from_resolved`): Text
      blocks → `messages[].content`; assistant `ToolUse` blocks →
      `messages[].tool_calls[]` (each `arguments` is `serde_json`-
      stringified, as the wire requires); consumer `ToolResult`
      blocks expand into separate `role: "tool"` messages
      addressed by `tool_call_id`; `tools[]` → top-level
      `tools[{type:"function", function:{name, description,
      parameters}}]`. `stream: true` always; `stream_options.
      include_usage: true` so the upstream emits a final usage
      chunk.
    - Response mapping (`mapper::ChunkAccumulator`): SSE chunks
      parsed via `eventsource-stream`; text deltas pass through
      as `TokenEventV2::Text`; tool-call deltas accumulate per
      `index` slot until `finish_reason: tool_calls`, then emit
      buffered call as `TokenEventV2::ToolUse`; trailing chunk's
      `usage` populates the v2 `Done` frame. `finish_reason`
      maps `stop` → `EndTurn`, `length` → `MaxTokens`,
      `tool_calls` / `function_call` → `ToolUse`, missing →
      `Error` (translated to `BackendUnavailable` daemon-side).
    - Pre-stream errors (transport, non-2xx HTTP) surface as
      `GenerateError::Unavailable` per ADR 0007; mid-stream
      transport errors terminate the channel without `Done`
      (lifecycle layer synthesises `error` frame). The reqwest
      client is rustls-only (no OpenSSL on Windows). Default
      timeout: 5 minutes.
  Tests: 8 mapper unit tests (text round-trip, tool replay,
  tool-result expansion, attachment rejection, accumulator
  behaviour across single text deltas, tool calls split across
  many chunks, missing-finish_reason error mapping). 5 wiremock-
  based integration tests in `tests/openai_compat.rs` exercising
  the full HTTP+SSE round-trip including the no-API-key path.
  New optional deps: `reqwest 0.12` (rustls-tls + json + stream),
  `eventsource-stream 0.2`, `futures-util 0.3`. Dev-dep:
  `wiremock 0.6`. Wire selection between adapters lands in
  Phase 5B (real router policy).
- **inferd-proto: v2 type surface** under the new `v2::` module
  (per ADR 0015). `RequestV2`, `MessageV2`, `ContentBlock` (with
  `Text` / `Image` / `Audio` / `Video` / `ToolUse` / `ToolResult`
  / forward-compat `Unknown` variants), `Attachment` /
  `AttachmentKind`, `Tool` / `ToolCallId` / `ToolUseInput`,
  `ResponseV2` / `ResponseBlock` / `StopReasonV2` / `ErrorCodeV2`
  / `UsageV2`. `RequestV2::resolve()` validates structural
  constraints (non-empty messages, non-empty content arrays,
  unique attachment ids, unique tool names, all `attachment_id`
  references resolve — including those nested inside
  `ToolResult::content`). Sampling defaults are *not* applied at
  the proto layer in v2 — they're backend-specific (ADR 0015).
  19 tests cover round-trip serialisation of every variant, the
  ADR 0015 JSON examples verbatim, validation negative cases, and
  forward-compat parsing of unknown content-block types. v2 lives
  on a separate socket; this commit ships *types only*, no daemon
  binding yet (Phase 1B).
- **inferd-daemon: v2 socket binding** (Phase 1B per ADR 0015).
  New `lifecycle_v2` module with `serve_tcp_v2` / `serve_uds_v2`
  / `serve_named_pipe_v2` mirroring v1's accept-loop shape but
  parsing `RequestV2` / writing `ResponseV2`. New `--v2` /
  `--v2-addr` / `--v2-tcp` CLI flags (and `INFERD_V2*` env vars).
  When `--v2` is set the daemon binds the v2 endpoint alongside
  v1 on its own socket / pipe path: `infer.v2.sock` on Unix,
  `\\.\pipe\inferd-infer-v2` on Windows. `default_v2_addr()`
  resolves the platform default. F-8 first-frame TCP auth is
  reused identically to v1; admin socket stays shared. Shutdown
  signal is now fan-out: the same Ctrl-C/SIGTERM closes both v1
  and v2 listeners.
- **inferd-engine: `Backend` trait grows v2 surface** (Phase 2A
  per ADR 0015). New types `TokenEventV2` (with `Text` /
  `Thinking` / `ToolUse` / `Done` variants) and `TokenStreamV2`.
  New trait methods `generate_v2(ResolvedV2) -> Result<TokenStreamV2>`
  with default impl returning `Internal("v2 not supported")` and
  `capabilities() -> BackendCapabilities` (default-zero, advertises
  text-only v1 — existing `mock` and `llamacpp` impls compile
  unchanged). `BackendCapabilities` exposes `v2`, `vision`,
  `audio`, `video`, `tools`, `thinking` flags. `mock` adapter
  opts in to v2 + thinking and gains a `generate_v2` impl that
  reuses the existing token tape, mid-stream-drop, and
  pre-stream-error knobs but yields v2 frames. `llamacpp`
  adapter stays at trait default for now — Phase 3+ wires
  chat templating + mtmd before its v2 path can do anything
  useful.
- **inferd-daemon: v2 dispatch wired** (Phase 2A). The v2 socket
  now dispatches validated requests through the shared `Router`
  (one warm model serves both wire versions). Backends that
  don't advertise `BackendCapabilities::v2 == true` see their
  v2 requests rejected with `Error{Internal, "backend ... does
  not advertise v2 capability"}`. Pre-stream errors map
  `GenerateError` variants to the right `ErrorCodeV2`
  (`InvalidRequest` / `BackendUnavailable` / `Internal`).
  Mid-stream backend failure (no `Done` event) emits a synthetic
  terminal `Error{BackendUnavailable, ...}` so clients never
  hang on a half-stream. Admission gate is shared with v1 — a
  v2 in-flight request occupies the same slot a v1 one would.
  Eight integration tests in `crates/inferd-daemon/tests/v2_stub.rs`
  pin: streaming text+done, multimodal dispatch, dangling
  attachment, empty messages, malformed JSON, multi-request
  pipelining, pre-stream `Unavailable`, and mid-stream drop.

- **ADR 0016**: consumer decodes media before sending. Amends ADR
  0015 §"v2 Attachment" — the daemon does not link image / audio
  codec libraries (would have violated ADR 0006/0013). The wire
  carries already-decoded forms: raw RGB octets + `width` /
  `height` for images, float32 PCM samples + `sample_rate` for
  audio. `Attachment` becomes a serde-tagged enum with one variant
  per modality (Image / Audio / Video / Unknown for forward-
  compat). `AttachmentKind` is removed (the discriminant is
  implicit in the serde tag); `mime` field is removed (information
  it carried is now in the variant). `RequestV2::resolve()`
  tightens to verify *kind correspondence* — a `ContentBlock::Image`
  must reference an `Attachment::Image`. Affected files: rewritten
  `crates/inferd-proto/src/v2/attachment.rs`, request validation
  in `crates/inferd-proto/src/v2/request.rs`, and downstream
  consumers (chat-template renderer, integration tests).
- **inferd-engine: libmtmd FFI bridge** (Phase 3A per ADR 0015 +
  ADR 0016). New `crates/inferd-engine/cpp/CMakeLists.txt` wraps
  `vendor/llama.cpp` and adds a `mtmd` static library target
  (rebuilds the source list inline because tools/mtmd/CMakeLists.txt
  also defines CLI executables we don't ship and that depend on
  `llama-common`, which we don't build). `build.rs` drives the
  wrapper, links `mtmd → llama → ggml → ggml-base → ggml-cpu` in
  static order, and runs bindgen against `mtmd.h` (output:
  `OUT_DIR/mtmd_bindings.rs`). New private module
  `crates/inferd-engine/src/mtmd_ffi.rs` includes the generated
  bindings and reuses shared types from `crate::ffi` via bindgen's
  `blocklist_type` + `raw_line` directives so `llama_model` and
  friends aren't redeclared.
- **inferd-engine + inferd-daemon + inferd CLI: hardware-acceleration
  detection and reporting** (#77). New `AcceleratorKind`
  (`Cpu` / `Cuda` / `Metal` / `Vulkan` / `Rocm`) and `AcceleratorInfo`
  (`{kind, gpu_layers}`) types on `inferd-engine`; added to
  `BackendCapabilities`. The `llamacpp` adapter reports the
  compile-time GGML backend (decided by the active cargo feature —
  `cuda` / `metal` / `vulkan` / `rocm`, falling back to `cpu`)
  plus the runtime `n_gpu_layers` it was constructed with. The
  daemon publishes a new `Capabilities` admin status frame after
  backend construction (`{"status":"capabilities","backend":"llamacpp",
  "v2":...,"vision":...,"audio":...,"tools":...,"thinking":...,
  "accelerator":"cuda","gpu_layers":99}`) so subscribers can
  introspect posture without trial-and-error. `StatusBroadcaster`
  caches the latest capability frame in its own slot so one-shot
  subscribers (e.g. `inferd doctor`) receive it on connect even
  after `Ready`. `inferd doctor` now reads up to two frames and
  prints a `[ ok ] backend: ... accelerator=... gpu_layers=...`
  line. `inferd-client::AdminEvent` grows seven backwards-additive
  optional fields (`backend`, `v2`, `vision`, `audio`, `tools`,
  `thinking`, `accelerator`, `gpu_layers`); older clients deserialise
  unchanged. Detection is *compile-time only* in v0.2 — runtime
  GPU enumeration (PCI / IOKit / SetupAPI) is out of scope; the
  reported accelerator equals the cargo feature the binary was
  built with. RTX 50-series Blackwell support requires CUDA
  Toolkit 12.8+ at build time.
- **inferd-engine: tool-result rendering pairs by `tool_call_id`**
  (Phase 4B). The Gemma 4 chat-template renderer now walks the
  full `messages[]` once at the top of `render` to build a
  `tool_call_id -> tool_name` map across every prior `ToolUse`.
  When a `ToolResult` block renders, it pairs to its originating
  call via `tool_call_id` and emits
  `<|tool_response>response:NAME{KEY:VALUE,...}<tool_response|>`
  with the correct tool name even when `tools[]` has multiple
  entries. The single-tool fallback is now a last-ditch heuristic
  used only if `tool_call_id` is unknown *and* `tools.len() == 1`;
  otherwise the renderer falls through to raw content (Gemma
  treats it as freeform tool output) instead of guessing. 3 new
  byte-exact tests in `chat_template_gemma4.rs`: pairing across
  multiple tools (out-of-order results), full round-trip after
  an assistant `ToolUse` (asserts both the original call and the
  paired response survive in their respective turns), and the
  unknown-tool_call_id fallback path.
- **inferd-engine: streaming tool-use parser** (Phase 4A). New
  `crates/inferd-engine/src/llamacpp/tool_parser.rs` is a pure-Rust
  state machine that wraps the v2 generate token stream and
  detects:
    - `<|tool_call>call:NAME{KEY:<|"|>VALUE<|"|>,...}<tool_call|>`
      sequences → `Output::ToolUse{tool_call_id, name, input}`.
      Generated tool_call_ids are `tc-{N}` per generation; the
      counter ensures uniqueness across multiple calls in the
      same stream.
    - `<|think|>...<|/think|>` sequences (per
      `docs/thinking.mode.in.gemma.md`) →
      `Output::Thinking(delta)`. Daemon forwards as
      `ResponseBlock::Thinking { delta }`.
    - Everything else → `Output::Text(delta)`.
    - Malformed payloads (opener but body doesn't parse) →
      `Output::Malformed(reason)`. The adapter terminates the
      stream; the daemon's lifecycle_v2 will surface that as a
      terminal error frame.
  The parser handles split-across-token-boundary sentinels by
  holding any pending bytes that match a strict prefix of an
  opener or closer, so a tokenizer that emits `<|tool_` then
  `call>` is parsed correctly. 11 unit tests pin every state
  transition (synthetic streams; no real model needed).
  `LlamaCpp::generate_v2`'s sampler loop now feeds each piece
  through the parser and emits `TokenEventV2::Text` /
  `Thinking` / `ToolUse` per its decisions. When any `ToolUse`
  was emitted, the terminal Done has
  `stop_reason: StopReasonV2::ToolUse` (per ADR 0015).
- **inferd-engine: Tier 3 v2 multimodal smoke** (Phase 3B). New
  `crates/inferd-engine/tests/llamacpp_multimodal.rs` exercises the
  v2 generate_v2 path against a real Gemma 4 GGUF + mmproj. Two
  tests:
    - `v2_text_only_streams_to_done` — confirms the v2 dispatch
      path works against a multimodal-capable backend with a
      text-only request. Asserts capabilities advertise v2 and
      Done is `EndTurn` or `MaxTokens` with non-zero usage counts.
    - `v2_image_attachment_round_trips` — decodes a JPEG/PNG to
      raw RGB via the `image` crate (consumer-side, per ADR
      0016), wraps it in `Attachment::Image` with width/height,
      sends through the v2 wire, asserts the multimodal prompt
      results in input_tokens > 50 (vs ~10 for text-only).
  Both tests gated behind the existing `llamacpp-integration`
  feature; skip with explanatory message when
  `INFERD_TEST_MODEL_PATH` / `INFERD_TEST_MMPROJ_PATH` /
  `INFERD_TEST_MULTIMODAL_IMAGE` are unset. Skip-on-vision-cap
  branch handles the case where the loaded mmproj is
  audio-only. `image` 0.25 (jpeg + png features only) added as
  a dev-dependency. Tests are deliberately not asserting on
  generated text content — that's fragile across quants and
  seeds; the contract under test is "wire round-trip survives
  end to end."
- **inferd-engine: `LlamaCpp::generate_v2`** (Phase 3A part 2).
  The llamacpp adapter now serves v2 requests end-to-end when
  configured with an `mmproj_path`:
    - `LlamaCppConfig` gains `mmproj_path` + `mmproj_sha256`.
    - `State` holds an `Option<Mtmd>` plus a cached capability
      snapshot (vision / audio / audio_sample_rate, probed at
      construction).
    - `Backend::capabilities()` advertises v2 + tools + thinking
      (Gemma 4 baseline) when an mmproj is loaded; vision and
      audio reflect what the mmproj's projector actually
      supports. No mmproj => default-zero caps (text-only).
    - `Backend::generate_v2()`: renders prompt + ordered
      attachments via `Gemma4Renderer`, base64-decodes each
      attachment's bytes into a `Bitmap` (raw RGB or f32 PCM per
      ADR 0016), runs `Mtmd::tokenize` + `mtmd_helper_eval_chunks`
      to fill the KV cache from the multimodal prompt, then runs
      a token sampler loop emitting `TokenEventV2::Text` deltas
      and a terminal `Done` (EndTurn or MaxTokens) with `UsageV2`
      input/output token counts. Pre-stream errors from any of
      these stages map to `GenerateError`; mid-stream backend
      failures terminate the channel without `Done` so the
      lifecycle layer can synthesise a terminal `Error` per ADR
      0007.
    - `mtmd::Mtmd::eval_chunks` (new): safe wrapper over
      `mtmd_helper_eval_chunks`. Picks up upstream's gemma-3
      non-causal mask handling, per-chunk batching, and decode
      error forwarding.
    - bindgen now also generates `mtmd-helper.h` bindings (the
      output file is unchanged at `OUT_DIR/mtmd_bindings.rs` —
      bindgen merges both headers into one generated module).
    - `inferd-engine` gains `base64` (~10 KB) and `serde_json`
      dependencies for the v2 path.
    - `crates/inferd-engine/src/llamacpp/chat_template/` is the
      new home of the Gemma 4 renderer + tests (moved from
      `crates/inferd-daemon/src/chat_template/`). The renderer
      is an engine-level concern: it shapes prompts for a
      specific engine, not gateway logic. The integration test
      `crates/inferd-engine/tests/chat_template_gemma4.rs` is
      gated behind `#![cfg(feature = "llamacpp")]` so default-
      feature builds (which don't link the engine) skip it.
    - The daemon's `lifecycle_v2` already dispatches v2 to the
      backend's `generate_v2`; this commit just makes that
      backend-side path actually do something instead of
      returning `Internal("not supported")`.
- **inferd-engine: safe Rust mtmd wrapper**. New
  `crates/inferd-engine/src/llamacpp/mtmd.rs` exposes:
  `Mtmd` (owning `mtmd_context`, supports `tokenize` plus
  `supports_vision` / `supports_audio` /
  `audio_sample_rate` capability probes), `Bitmap` (image RGB or
  audio f32 PCM, owning `mtmd_bitmap`, with id-set helper for
  upstream's KV-cache de-duplication), `InputChunks` (owning
  collection populated by `tokenize`), `InputChunk<'_>` (borrow,
  `kind`/`n_tokens`/`n_pos`/`id`), `MmprojCaps` +
  `probe_mmproj_caps` (capability probe without instantiating a
  full context), `default_media_marker` (the `<__media__>`
  literal mtmd injects fences around). All types `Send + Sync`,
  Drop calls the matching mtmd_*_free. Safe wrapper does not yet
  call mtmd_encode_chunk — that lives in the `LlamaCpp` adapter's
  Phase 3A-followup work where `generate_v2` actually splices
  encoded embeddings into `llama_decode`. The bridge is the
  testable boundary; the adapter's encode loop is the next
  commit.
- **inferd-daemon: chat-template renderer** (Phase 2B per ADR
  0013/0015). New `chat_template` module with a `Gemma4Renderer`
  that translates a `ResolvedV2` into the byte-exact Gemma 4
  prompt format (`<|turn>system\n...<turn|>`,
  `<|tool>declaration:...<tool|>`,
  `<|tool_call>call:NAME{...}<tool_call|>`,
  `<|tool_response>response:NAME{...}<tool_response|>`) plus an
  ordered list of attachments referenced by `<__media__>` markers
  in the rendered text — exactly the shape libmtmd's
  `mtmd_tokenize` consumes. Phase 3A wires it into the
  `LlamaCpp` adapter; until then, the renderer ships standalone
  and is exercised by 9 byte-exact integration tests in
  `crates/inferd-daemon/tests/chat_template_gemma4.rs` against
  the canonical examples from
  `docs/text.function.calling.with.gemma.4.md`. Tools without a
  system message synthesise an empty system turn (matching
  upstream); ToolResult content blocks render inside the model
  turn (matching upstream's `<|tool_response>...<tool_response|>`
  inline form, not as a separate turn). Schema and inline
  argument rendering replaces JSON `"..."` quoting with Gemma's
  `<|"|>...<|"|>` special-token form so the tokenizer routes
  string literals correctly.

### Changed (cherry-picked from v0.1.13 on main, 2026-05-20)

- **`crates/inferd-engine/src/llamacpp/loader.rs`**: when an
  `expected_sha256` is supplied, stream-hash the model file
  in place at its original path and hand that same path to
  `llama_model_load_from_file`. No more daemon-owned tempdir
  copy. Closes issue #6 — WSL2 / tmpfs-constrained hosts could
  not load multi-GB GGUFs on cold start because the defensive
  temp-copy doubled disk usage. F-6 status flipped from
  "mitigated" to "accepted" with the rationale that an attacker
  with write access to the user's model file in the
  microseconds between hash and mmap has a strictly larger
  threat than hashing can defend against — same justification
  F-6 already applied to the no-hash path.
- **`THREAT_MODEL.md` F-6** updated to reflect the new posture.
- **`crates/inferd-engine/Cargo.toml`** drops runtime `tempfile`
  dependency.

## [0.1.12] - 2026-05-20

Hotfix: v0.1.11 macOS tarball was missing
`packaging/launchd/install-launchagent.sh` and
`packaging/launchd/uninstall-launchagent.sh`. The scripts existed
in the source tree (added in 6095a2e) but the release workflow's
macOS staging step only copied the plist. Anyone unpacking the
v0.1.11 tarball on macOS could not run the documented install
flow without grabbing the scripts from the repo separately.

### Fixed

- **`.github/workflows/release.yml`**: macOS staging step now
  copies `install-launchagent.sh` and `uninstall-launchagent.sh`
  alongside the plist, with `chmod +x` applied so they're
  directly executable from the unpacked tarball.

## [0.1.11] - 2026-05-20

Tarball-only release. Crates on crates.io stay at 0.1.9 (no
wire-surface change). The point is to ship the macOS LaunchAgent
fix to thlibo without waiting for the v0.2 cycle to land.

### Fixed

- **macOS LaunchAgent plist** (`packaging/launchd/io.inferd.daemon.plist`):
  four bugs from the initial Windows-authored drop are now corrected:
  (1) `USERNAME_HERE` hardcoded paths replaced with `__HOME__`/`__TMPDIR__`
  install-time placeholders (launchd does not expand `$HOME`, `${HOME}`, `~`,
  or `$TMPDIR` in plist values — empirically verified);
  (2) `--admin-addr` argument added so the daemon binds the admin socket at
  `$TMPDIR/inferd/admin.sock`, matching `default_admin_addr()` and the Go
  client default;
  (3) `--backend mock` removed from the shipped plist (it was a copy-paste
  from CI; the real unit should use the configured backend);
  (4) wrong socket/lock dir (`Application Support`) replaced with `$TMPDIR`
  paths — sockets must be in a directory the daemon user owns and that
  survives the session but not cross-user.
- **Daemon runtime dir auto-create** (`crates/inferd-daemon/src/main.rs`):
  the daemon now calls `fs::create_dir_all` on the lock file's parent
  directory before acquiring the lock. On macOS, `$TMPDIR/inferd/` is not
  pre-created by launchd (unlike Linux where `RuntimeDirectory=` in the
  systemd unit handles this); without the mkdir the daemon would fail with
  "lock acquire failed: No such file or directory" on a clean login.

### Added

- `packaging/launchd/install-launchagent.sh` — one-shot install script that
  substitutes `__HOME__`, `__TMPDIR__`, and `__BIN__` placeholders with
  runtime values (`getconf DARWIN_USER_TEMP_DIR` for TMPDIR), creates the
  log directory, bootstraps the LaunchAgent, and enables it. Accepts an
  optional binary path argument (default `/usr/local/bin/inferd-daemon`).
- `packaging/launchd/uninstall-launchagent.sh` — symmetric teardown: stops,
  boots out, disables, and removes the installed plist.
- `packaging/README.md` updated to point at the install scripts instead of
  the now-removed "edit `USERNAME_HERE` by hand" instruction.

### Documentation

- **ADR 0013**: inferd is the gateway, not the pipe. Locks the
  architectural posture: the daemon owns model-specific shaping
  (chat templates, attachment routing, tool-call orchestration);
  middleware sends semantic intent. Corrects an earlier framing
  in the v0.1.x cycle that called inferd a "pipe" — the pipe
  framing breaks against llama.cpp's mtmd interface and against
  every other LLM gateway's expected shape.
- **ADR 0014**: the inferd CLI is a reference middleware, not a
  privileged surface. The `crates/inferd/` binary uses the same
  public crates (`inferd-client`, `inferd-daemon` lib) any
  external consumer would. No private daemon API, no internal
  subcommands. Every CLI feature is implicitly a contract test
  for the public library surface.
- **ADR 0015**: v2 wire protocol shape — typed content blocks
  (text / image / audio / video / tool_use / tool_result),
  top-level `attachments[]` carrying raw bytes referenced by
  blocks, top-level `tools[]` for function definitions.
  Anthropic-API-shaped, lives on a separate socket per ADR 0008
  so v1 stays untouched. **Design only — no code in this
  release.** v2 ships as part of v0.2 work.
- **`INTEGRATING.md`** rewritten opening + new "v0.2 preview"
  section. Frames inferd as a gateway with the mental-model
  diagram middleware authors recognise from Anthropic /
  OpenAI / Bedrock APIs. Concrete example of the v2 wire
  shape so consumers writing v0.1 code today can plan ahead.

## [0.1.10] - 2026-05-20

### Changed

- **`inferdctl` renamed to `inferd`.** Single CLI binary in the
  gh / kubectl shape — same subcommands (`status`, `watch`,
  `pull`, `doctor`), shorter command, room to grow. Pairs with
  `inferd-daemon` exactly the way `kubectl` pairs with `kubelet`.
  Crate moved from `crates/inferdctl/` → `crates/inferd/`. Bin
  name in release tarballs is now `inferd` / `inferd.exe`.
- **`inferd-stdio` crate retired.** The previously-scaffolded
  stdio-shape variant (`-p "..."` prompt mode for one-shot
  invocations) no longer ships as its own binary. When that
  shape lands, it'll be the `inferd` CLI's default subcommand
  — i.e. `inferd -p "hello"` rather than a separate
  `inferd-stdio "hello"`. One binary, many shapes. The empty
  Cargo.toml + README scaffold under `crates/inferd-stdio/`
  was removed.

### Migration

- Operators / consumers using v0.1.9: rename `inferdctl` →
  `inferd` in scripts. Subcommand surface unchanged.
- The release tarball still ships `inferd-daemon` for the
  long-running service; `inferd` is the new shorter name for
  what was `inferdctl`.

## [0.1.9] - 2026-05-20

Closes the last protocol-promise gap and adds the operator CLI
that's been on the punch list since the start of v0.1.

### Added

- **Admission queue wired into the lifecycle.** The wire spec has
  promised `Response::Error{code: queue_full}` frames since
  alpha.0; the daemon never emitted them. Today's daemon now
  enforces a global capacity of `active_permits + queue_depth`
  outstanding requests across all connections via a shared
  `tokio::Semaphore`. The (capacity+1)th request gets a clean
  `queue_full` frame and the connection moves on. Two new
  integration tests in `tests/queue_full.rs` pin the behaviour;
  the existing concurrency stress tests in `tests/stress.rs`
  still pass with admission disabled.
  Closes the architectural-promise gap that's been tracked since
  the v0.1.0-alpha.0 design notes flagged "queue module exists
  but isn't wired."
- **`inferdctl` operator CLI** at `crates/inferdctl/`. Four
  subcommands:
  - `inferdctl status` — one-shot admin snapshot as JSON; exits
    0 on `ready`, non-zero otherwise. For shell scripts.
  - `inferdctl watch` — stream admin events forever. For
    operators watching first-boot model download.
  - `inferdctl pull` — read `~/.inferd/config.json`, fetch the
    configured model into the CAS store
    (`$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`), verify
    SHA-256 with constant-time compare, write the manifest.
    Bypasses the daemon. Idempotent.
  - `inferdctl doctor` — diagnose connectivity. Checks config,
    blob, manifest, admin socket. Prints a punch list with
    `[ ok ]` / `[FAIL]` markers.

  Bundled in every release tarball alongside `inferd-daemon`.

### Changed

- `lifecycle::AcceptContext` gained an optional `admission` field.
  Tests pass `None` to keep the old "every request admitted"
  semantics; production wires a real `Admission` from the
  daemon's `--active-permits` / `--queue-depth` CLI flags
  (defaults: 1 active, 10 queued).
- `crates/inferd-daemon/src/queue.rs` rewritten. The previous
  `Queue<T>` type was a generic mpsc + semaphore that anticipated
  a worker-loop pattern that never materialised. Replaced with
  a much simpler `Admission` type: one shared semaphore sized at
  `active + queued` total slots, `try_admit()` returns an
  `OwnedSemaphorePermit` the connection task holds for the
  generation's duration. Drops on completion / cancel / EOF.

## [0.1.8] - 2026-05-19

The "actually shippable" release. **First non-alpha publish to
crates.io** since `0.1.0-alpha.0`. Closes the gap that left
consumers (e.g. thlibo) stuck because `cargo add inferd-client`
resolved to a pre-release version that requires explicit
`=0.1.0-alpha.0` pinning.

### Added

- **`inferd-proto 0.1.8` and `inferd-client 0.1.8` published to
  crates.io.** `cargo add inferd-client` now resolves to a
  non-alpha version. Wire schema and client surface unchanged
  from alpha.0 — same `Request`/`Response`/admin envelope.
- **`INTEGRATING.md`** at the repo root: end-to-end "how to use
  inferd from your own product" guide. Covers install, config,
  per-language client examples (Rust + Go), Pattern A vs B
  readiness, error contract, gotchas. Designed so a consumer
  can copy snippets and have them work.
- **`crates/inferd-client/README.md`**: expanded for crates.io
  rendering. Explicit "install the daemon first" preamble,
  per-platform endpoint paths, version-resolution semantics
  spelled out so consumers know `0.1` works.
- **Stdout download progress logging** (closes #3). `fetch_model`
  emits `model download starting` / `progress` (every 32 MiB or
  5 s) / `complete` log lines so an operator running the daemon
  manually sees the 5 GB pull is alive. Mirrors the admin-socket
  event cadence; subscribers and log tailers see the same
  numbers.

### Held / not in this release

- **Windows arm64 release tarball** — the v0.1.7 attempt failed
  on the `windows-11-arm` runner's llamacpp build. Per user,
  not a GA gate; revisited when there's appetite to debug the
  arm64 runner image. v0.1.7 was never tagged.

## [0.1.7] - 2026-05-19

Adds Windows arm64 to the release matrix. Five released targets:

  - x86_64-unknown-linux-gnu
  - aarch64-unknown-linux-gnu
  - aarch64-apple-darwin
  - x86_64-pc-windows-msvc
  - aarch64-pc-windows-msvc *(new)*

### Added

- **Windows arm64 release tarball.** Built natively on
  `windows-11-arm` (free for public repos since 2025), same
  formula as the x86_64 Windows job — no `cross`, no foreign-
  target C++ toolchain. Closes the last platform gap a v0.1.x
  user could plausibly hit.
- **CI runs on `windows-11-arm` too.** All four Windows-touching
  jobs (default, llamacpp feature, Tier 5 security, go client)
  now matrix in arm64 alongside x64. Catches arm64-specific
  build issues at PR time, not release time.

## [0.1.6] - 2026-05-19

The binary-size guard added in v0.1.4 (mac claude's commit
`177b0c1`) had a false-positive: it assumed a real-llamacpp Linux
binary would be ≥10 MB, but stripped Linux release binaries with
statically-linked libllama come in around 9 MB. v0.1.4 + v0.1.5
both failed the guard despite producing correctly-built binaries
that did real inference.

Verified directly: a 9.3MB WSL Linux build with the v0.1.5 source
returns real Gemma 4 tokens via `--backend llamacpp` against a
real GGUF. The guard threshold was wrong, not the build.

### Fixed

- **Replace size-based guard with `--help` substring check.**
  Direct test: if `BackendKind::Llamacpp` got compiled out, clap's
  value-enum doesn't list `llamacpp` and `--help` won't mention
  it. No false positives from stripped-binary size variation.

## [0.1.5] - 2026-05-19

The release-tarball saga continues. v0.1.4 was tagged with the
right feature flag (`inferd-daemon/llamacpp`) but the build
job's binary-size guard caught a 9.2MB binary on Linux x64 —
under the 10MB threshold for a real-llamacpp build. The tag
exists, no release artifacts were attached. Investigation
showed Swatinem/rust-cache served a stale target directory
even with a feature-suffixed cache key.

The clean fix: stop caching the release builds. Releases are
rare, the binary-size guard catches the failure mode, and
adding ~5 min per platform per release is a fair trade for
never shipping mock-only tarballs again.

### Fixed

- **Disable rust-cache for release builds.** Each release does
  a clean build from scratch. The Swatinem cache served stale
  mock-only target dirs in v0.1.1 and v0.1.4 despite
  feature-suffixed keys; rather than chase the cache key that
  always works, we just don't cache the release path. Verified
  Linux x86_64 end-to-end with real Gemma 4 inference in WSL2.

### Verified end-to-end (real inference, not mock)

- **Linux x86_64**: TTFT 670ms, 13 tokens, generated text
  semantically correct, `backend: "llamacpp"`, `stop_reason:
  "end"`. Closes the gap mac claude flagged on alpha.0 and
  thlibo claude flagged on v0.1.1.
- **Windows x86_64**: validated earlier in the v0.1.x cycle.
- **macOS aarch64**: tracked under issue #2 — pending mac
  claude validating the v0.1.5 tarball.
- **Linux aarch64**: untested (no arm64 hardware locally).

## [0.1.3] - 2026-05-19

Release-tooling fix only. Crates unchanged from 0.1.1; **no cargo
publish.** The point of the release is to get a real-inference
binary into the aarch64-linux tarball that 0.1.1 shipped mock-only.

(0.1.2 was tagged but never released — the publish step failed on
unresolvable Action versions before any artifacts were attached.
0.1.3 lands the fix.)

### Fixed

- **aarch64-linux release tarball ships with `--features llamacpp`.**
  release.yml's aarch64 job switched from `cross` (which couldn't
  configure a foreign-target C++ toolchain for llama.cpp's cmake
  build) to GitHub's native arm64 runner (`ubuntu-24.04-arm`,
  free for public repos since January 2025). Same `cargo build
  --features llamacpp` formula as the other targets — no special
  cases.

### Changed

- **GitHub Actions versions bumped** to current latest available
  major-version tags: `actions/upload-artifact@v4` → `@v7`,
  `actions/download-artifact@v4` → `@v8`. Both Node-24, closing the
  deprecation annotations from the 0.1.1 run.
  `sigstore/cosign-installer` and `softprops/action-gh-release`
  stay at `@v3`/`@v2` respectively — those projects publish 4.x /
  3.x point releases but haven't tagged a new major-version
  float, so the floats remain on v3 / v2.
- **Go version** in CI changed from pinned `1.21` to `stable`.
  Tracks current stable; the Go module's `go 1.21` directive is
  unchanged so external Go consumers on 1.21+ still work.
- **setup-go cache disabled** for the Go client job — there's no
  `go.sum` (zero external deps) so cache had nothing to key off
  and emitted a cosmetic miss annotation on every run.

## [0.1.1] - 2026-05-19

First non-alpha release. Drops the `-alpha` suffix because:

- The release tarball now ships a binary that does real inference
  (Linux x86_64, macOS aarch64, Windows x86_64 — all with
  `--features llamacpp`). Previous alpha tarball was mock-only,
  reported by an external integrator. aarch64-linux still ships
  mock-only pending a working cross-build for the C++ toolchain.
- Cross-platform validation passed across Windows + macOS +
  Linux + WSL2 systemd, including a 50-client concurrency
  stress test, mid-stream cancellation, in-flight shutdown,
  and 200-cycle connect churn.
- The Windows named-pipe DACL is now SID-restricted at the
  kernel-object level (F-7), not relying on default
  CreateNamedPipe behaviour.
- Documented multi-model decision (ADR 0012) means there are no
  open architectural questions for v0.x.

Known gap: the admission queue defined in
`crates/inferd-daemon/src/queue.rs` is not yet wired into
`handle_connection`. Today each connection runs its request
handling inline, so the protocol-promised `queue_full` error frame
is never emitted. With the llamacpp backend, concurrent requests
serialise on the inner mutex (correct behaviour, just silent
instead of `queue_full`-fronted). Tracked for v0.1.2.

### Added

- **Edition 2024 + Rust 1.95.** Workspace migrated; let-chains
  collapsed in three sites (`config_file::expand_paths`,
  `lifecycle::handle_connection`, `store::ModelStore::open`).
- **Concurrency stress harness** at
  `crates/inferd-daemon/tests/stress.rs`. Four tests covering
  50-client saturation, mid-stream disconnect resilience,
  graceful shutdown with jobs in-flight, and accept-loop pressure.
  Uses the new `MockConfig::token_delay_ms` field so requests
  overlap on the wire.
- **ADR 0012**: one warm model per inferd process. Closes the
  multi-model question that v0.1's plan flagged as a non-goal —
  multi-model warm pooling is rejected for the foreseeable v0.x
  cadence on lean-core (ADR 0006) and protocol-cost (ADR 0008)
  grounds. Operators who need N concurrent models run N inferd
  processes. The router (ADR 0007) multiplexes *backends*, not
  *models*.

### Changed

- **release.yml builds with `--features inferd-engine/llamacpp`**
  on ubuntu-latest x86_64, macos-latest aarch64, and
  windows-latest x86_64. Closes the alpha tarball gap where the
  shipped binary couldn't run real inference. aarch64-linux still
  builds mock-only via `cross` because the cross image lacks the
  C++ toolchain configuration for foreign-target cmake.
- **systemd unit**: dropped F-16 hardening directives that need
  `CAP_SYS_ADMIN` (`PrivateTmp`, `PrivateDevices`,
  `ProtectSystem=strict`, `ProtectControlGroups`, `ProtectKernel*`,
  `RestrictNamespaces`, `MemoryDenyWriteExecute`,
  `CapabilityBoundingSet`, `AmbientCapabilities`). They fail
  unit-level validation on `systemctl --user` with
  `status=218/CAPABILITIES` because a non-root user has no
  capabilities to bound or grant. The remaining set is the maximal
  subset that works without root. A future
  `inferd.service.system` template will ship the full F-16
  hardening for system-unit deployments.

### Security

- **F-7 Windows hardening**: named pipes are now created with an
  explicit SDDL DACL (`O:<sid>G:<sid>D:P(A;;GA;;;<sid>)`) that
  grants `GENERIC_ALL` to the daemon's own user SID and nobody
  else (protected DACL, no inheritance). Closes the documented
  alpha.1 gap where the pipes relied on the default
  `CreateNamedPipe` posture (creating-user-only by accident, not
  by guarantee). Implementation:
  `crates/inferd-daemon/src/windows_security.rs::
  PipeSecurityDescriptor` plus
  `ServerOptions::create_with_security_attributes_raw` in both
  `bind_named_pipe` and `bind_admin_pipe`.

### CI

- **systemd-unit smoke job** on `ubuntu-latest`. Boots the daemon
  through `systemctl --user` with the shipped unit file, verifies
  socket modes (0600 admin, 0660 inference), drives an NDJSON
  request through the inference UDS, asserts the journal contains
  no crash-loop containment trips. Closes the WSL2-systemd gap
  flagged in the Linux runtime handoff §6.

## [0.1.0-alpha.0] - 2026-05-19

First crates.io release.

### Released

- **`inferd-proto` 0.1.0-alpha.0** on crates.io. Wire format types
  (`Request`, `Response`, `Message`, `ErrorCode`, `StopReason`),
  NDJSON framing with 64 MiB per-frame cap. Canonical schema for
  any-language clients.
- **`inferd-client` 0.1.0-alpha.0** on crates.io. NDJSON-over-IPC
  client (UDS / Windows named pipe / loopback TCP), admin event
  subscriber, retry-and-wait helpers (Pattern A passive +
  Pattern B active). Re-exports `inferd-proto` so consumers don't
  need both deps.
- Both crates pinned to `inferd-daemon 0.1.0-alpha.0` via `=`-strict
  versioning so the wire-protocol contract is enforced at the
  Cargo.lock layer.

### Fixed

- **Linux runtime path defaults**: `default_admin_addr()` (daemon
  + `inferd-client`) and `DefaultAdminAddr()` (Go client) now resolve
  the Linux admin-socket path via `$XDG_RUNTIME_DIR/inferd/admin.sock`
  with fallback chain `$HOME/.inferd/run/` → `/tmp/inferd-<uid>/`.
  The previous literal `/run/inferd/admin.sock` is root-only and
  was incompatible with `systemd --user` units (per the Linux
  runtime handoff). `docs/protocol-v1.md` now freezes the
  resolution algorithm rather than a literal path.
- **systemd unit**: `packaging/systemd/inferd.service` now passes
  `--admin-addr %t/inferd/admin.sock` explicitly, drops
  `--group inferd-users` from the default ExecStart (the group
  doesn't exist on a fresh install; default `RuntimeDirectory=`
  ownership is daemon-uid-only, which is the safer default; opt
  in for multi-user shared deployments), and adds
  `StartLimitBurst=3` / `StartLimitIntervalSec=60s` to contain
  crash-loops when assets are missing. Validated end-to-end on
  Ubuntu / WSL2: daemon comes up under `systemctl --user`,
  sockets bind at `/run/user/<uid>/inferd/{admin,infer}.sock`
  with modes `0600`/`0660`, NDJSON request returns a `done`
  frame.
- **README Linux install + WSL APE-binary advisory**: documents
  the `systemctl --user` install path and warns WSL users about
  stale Cosmopolitan-Libc binaries on `PATH` (`MZ` header tripping
  the `binfmt_misc` `WSLInterop` handler).
- **CI actions upgraded to Node 24**: `actions/checkout` → v6,
  `actions/setup-go` → v6 in both CI and release workflows.
- **Windows go e2e admin addr**: `testAdminAddr` returns a named-pipe
  path on Windows so `TestEndToEndAgainstDaemon` passes the right
  `--admin-addr` format on all three platforms.
- **llamacpp Linux link**: `build.rs` now links `-lgomp` on Linux so
  `GOMP_barrier`/`GOMP_parallel`/`omp_*` symbols from `ggml-cpu`'s
  OpenMP compilation resolve at link time.
- **llamacpp macOS link**: `build.rs` now links `ggml-blas` (static)
  and `Accelerate.framework` on macOS so `_ggml_backend_blas_reg` and
  `vDSP_*` symbols resolve.
- **Go e2e on Linux**: `TestEndToEndAgainstDaemon` now passes
  `--admin-addr` pointing to a temp-dir socket instead of relying on
  the platform default (`/run/inferd/admin.sock`), which requires root
  on Linux and caused the daemon to fail before binding its TCP port.
- **macOS build**: `peercred::unix::from_stream` now uses
  `sockopt::LocalPeerCred` + `sockopt::LocalPeerPid` (two separate
  `getsockopt` calls) on macOS/iOS. `sockopt::PeerCredentials`
  (`SO_PEERCRED`) is Linux/Android only in nix 0.27; the previous
  code failed to compile on macOS with an unresolved import error.

### Added

- **Shared content-addressable model store** ([ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md)).
  Models now live at `$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`
  with a `manifests/<name>.json` indirection layer and an advisory
  `locks/<name>.lock` per writer. Resolution order: `models_home`
  config field → `MODELS_HOME` env → platform default
  (`%LOCALAPPDATA%\models`, `~/.local/share/models`, `~/Library/
  Application Support/models`). Wire-compatible with the cross-tool
  *Shared Local Model Store* convention so other tools that adopt
  it can share the same blobs.
- `crates/inferd-daemon/src/store.rs` — owns CAS path resolution,
  manifest read/write (atomic via `<file>.tmp` + rename), and the
  quarantine directory for SHA-mismatched bytes.

### Changed

- `crates/inferd-daemon/src/fetch.rs` — `fetch_model` now writes
  into the CAS layout: streaming download into `.partial-<hash>/
  data.tmp`, constant-time SHA verify (F-5), atomic rename into
  `<aa>/<hash>/data`, then manifest write last. Acquires
  `LOCK_EX` on `locks/<name>.lock` for the duration. The function
  signature now takes `&ModelStore` instead of `&Path`.
- `crates/inferd-daemon/src/config_file.rs` — `models_dir` field
  removed; replaced with `models_home` (optional override of
  `$MODELS_HOME`). The `model` block dropped its `filename` field
  because the on-disk path is now derived from the SHA.

### Documentation

- `README.md`, `CLAUDE.md`, `context.md`, `THREAT_MODEL.md`,
  `docs/plan-v0.1.md`, `CONTRIBUTING.md` reframed as a standalone
  service. Reference consumers (e.g. middleware projects) are
  examples of clients, not parents — inferd does not encode any
  consumer's assumptions. ADR bodies (immutable) are unchanged.

## [0.1.0-alpha.2] - 2026-05-16

Closes the three security follow-ups identified in alpha.1's
"Not yet verified" / "Post-alpha tracked work" buckets: F-7
peer credentials, F-8 TCP API-key auth, F-16 daemon hardening
manifests.

### Added

- **F-7 (peer credentials)** — `crates/inferd-daemon/src/peercred.rs`.
  `PeerIdentity` struct extracted on every accept and recorded on
  the `connection_accepted` activity-log event. Unix path uses
  `nix::sys::socket::getsockopt(PeerCredentials)`
  (`SO_PEERCRED`/`LOCAL_PEERCRED`); Windows path uses
  `GetNamedPipeClientProcessId` →
  `OpenProcessToken(TOKEN_QUERY)` →
  `GetTokenInformation(TokenUser)` →
  `ConvertSidToStringSidW`. Loopback TCP gets a degraded
  `from_tcp(remote_addr)` for log correlation; the real perimeter
  comes from F-8.
- **F-8 (TCP API key)** — `crates/inferd-daemon/src/auth.rs`.
  When `AcceptContext::expected_api_key` is `Some`, every TCP
  connection must send `{"type":"auth","key":"..."}` as its
  first NDJSON frame. Constant-time compare via
  `subtle::ConstantTimeEq`. Missing or wrong key closes the
  connection silently — no protocol error frame, no endpoint
  confirmation. New `--api-key` / `INFERD_API_KEY` flag.
- **F-16 (hardening manifests)** — `packaging/`.
  `systemd/inferd.service` (per-user, full hardening directive
  set), `launchd/io.inferd.daemon.plist` (LaunchAgent),
  `windows/install.ps1` (sc.exe with NetworkService).
  `release.yml` bundles the matching manifest into each
  per-platform release archive.
- `lifecycle::AcceptContext` struct: per-accept policy bucket
  threaded through `serve_tcp` / `serve_uds` / `serve_named_pipe`
  into `handle_connection`. Future per-connection policy (rate
  limits, per-caller quotas) extends this rather than each
  signature.

### Fixed

- `lifecycle::read_frame_async` previously wrapped its input in a
  fresh `BufReader` on every call. Bytes the fresh wrapper
  prefetched past the current line were lost when it dropped.
  Surfaced as a "request frame lost after auth" symptom in F-8
  testing. Both `read_auth_frame` and `read_frame_async` now take
  the caller's `AsyncBufRead` directly, consuming from the shared
  per-connection buffer.

### Changed

- `lifecycle::handle_connection` signature: gains `peer:
  PeerIdentity` and `ctx: AcceptContext` parameters.
  `serve_tcp` / `serve_uds` / `serve_named_pipe` likewise take
  `AcceptContext`. Tests updated.
- `crates/inferd-daemon` crate-level lint posture:
  `forbid(unsafe_code)` → `deny(unsafe_code)` so the platform-
  specific `peercred` submodules can scope an inner
  `allow(unsafe_code)` for the FFI surface. Every other module
  in the daemon remains unsafe-free.
- `windows-sys` features bumped: added
  `Win32_Security_Authorization` and `Win32_System_Memory` for
  `ConvertSidToStringSidW` and `LocalFree`.

### Security

- THREAT_MODEL F-7, F-8 → mitigated with named code sites and
  verifying tests.
- THREAT_MODEL F-16 → mitigated on Linux + macOS; Windows
  partial (service-ACL SDDL is post-alpha).
- All other findings unchanged from alpha.1.

### Verified

- 74/74 Rust tests pass on Windows (was 67 in alpha.1; +5 daemon
  unit tests under `auth::tests`, +4 integration tests in
  `tests/auth.rs`, -2 attribution).
- Workspace clippy `-D warnings` clean. fmt clean.

### Not yet verified

Same list as alpha.1 — real Gemma 4 GGUF run, CI on real
Actions runners, Linux/macOS test execution.

## [0.1.0-alpha.1] - 2026-05-16

First tagged drop. Code-complete for v0.1's planned scope plus
the pieces of M4 that landed before alpha; F-7/F-8/F-16 are the
known follow-ups (see `docs/plan-v0.1.md` §"Post-alpha tracked
work").

### Added

#### Crates

- `inferd-proto` — wire format. `Request`/`Resolved` with Gemma 4
  sampling defaults. `Response` enum with `stop_reason`,
  `backend` on `done`, structured `code` on `error` per ADR
  0008. `read_frame` / `write_frame` with a 64 MiB bounded
  reader (THREAT_MODEL F-1 mitigated). 15 tests.
- `inferd-engine` — `Backend` async trait, `TokenEvent` /
  `TokenStream`, `GenerateError`. `mock` adapter with
  failure-mode injection. `llamacpp::LlamaCpp` adapter behind
  the `llamacpp` cargo feature: model load with constant-time
  SHA-256 verification (F-5), `llama_context` allocation, decode
  + sample loop on `spawn_blocking`, GBNF wired to
  `llama_sampler_init_grammar`, cancellation by drop. 9 default
  tests + 3 tier-3 stubs that skip without
  `INFERD_TEST_MODEL_PATH`.
- `inferd-daemon` — binary. Lifecycle, single-instance lock with
  symlink rejection (F-2), bounded admission queue (1 active +
  10 queued, non-blocking submit, `code: queue_full`), no-op
  `Router` (ADR 0007 shape ready for v0.2), UDS / loopback TCP /
  Windows named-pipe endpoints, ready-gated listener creation
  (F-13), `clap`-driven CLI. 35 unit tests + 4 + 2 + 2
  integration tests.
- `inferd-stdio` — Cargo.toml scaffold only; sources land when a
  caller needs the stdio variant.

#### Activity log

- `LogxWriter` rotating NDJSON writer (3 generations, F-4)
  with a write-time `redact_in_place` redactor (F-3) covering
  Authorization headers, key=value secrets, JWTs, AWS
  AKIA/ASIA, Slack `xox*`, GitHub `gh*_`, Cisco Things
  `pat-`/`thingspat_`, OpenAI `sk-`.
- `LogxLayer` `tracing_subscriber::Layer` serialising events as
  NDJSON (`t`, `level`, `component`, `msg`, structured fields).
- `lifecycle::handle_connection` emits `request_done` /
  `request_error_mid_stream` events per request.

#### Build + release

- `vendor/llama.cpp` submodule pinned at tag `b9159` (commit
  `5c0e94683`, 2026-05-15).
- `inferd-engine/build.rs` runs CMake on the submodule under
  feature `llamacpp` with server/CLI/examples/tools/curl off,
  static-lib output, release CRT (Windows). Generates Rust
  bindings via bindgen 0.71. GPU backends as opt-in cargo
  features (`cuda`, `metal`, `vulkan`, `rocm`).
- `.github/workflows/ci.yml` — fmt + clippy + test on
  `[ubuntu, macos, windows]` with and without the `llamacpp`
  feature. Go-client job builds the daemon binary then runs
  `go vet` + `go test`. `cargo audit` on push-to-main +
  schedule (does not block PRs).
- `.github/workflows/release.yml` — tag-triggered (`v*`) matrix
  build (linux x86_64, linux aarch64 via `cross`, macos
  aarch64, windows x86_64). Generates CycloneDX SBOM via
  `cargo cyclonedx`, signs each archive with keyless cosign
  (Sigstore OIDC), publishes to GitHub Release. F-15 mitigated.

#### Go client (M5)

- `clients/go/` Go module at
  `github.com/3rg0n/inferd/clients/go`. `Client` struct with
  `DialTCP`, `DialUDS` (Unix-only), `DialPipe` (Windows-only).
  `Generate(ctx, req)` returns a frame channel; `ctx` cancel
  closes the connection. Bounded reader at 64 MiB to mirror the
  Rust crate.
- `client_test.go` — protocol-shape round-trip + end-to-end
  against the live Rust daemon binary (auto-locates
  `<workspace>/target/debug/inferd-daemon[.exe]`; override with
  `INFERD_DAEMON_BIN`).

#### Documentation

- `docs/protocol-v1.md` — clean inferd-native wire spec per ADR
  0008.
- `docs/ai.internals.explained.md` — 15-component explainer of
  how local LLM serving stacks are built (standalone reference).
- `docs/test-strategy.md` — six test tiers, platform matrix,
  cargo features.
- `docs/adr/0001`–`0009` — full architectural decision record set.
  0001 superseded by 0008; 0003 superseded by 0005.
- `THREAT_MODEL.md` — 16 findings (F-1 through F-16) with
  per-finding mitigation status and code-site references.
- `CLAUDE.md` — guidance for future Claude Code sessions in
  this repository.
- `vendor/llama.cpp.PIN.md` — pinned commit + bump procedure.

### Changed

- Workspace MSRV: floor of 1.89 (uses `std::fs::File::try_lock`,
  stable since 1.89).
- ADR 0001 → `superseded by 0008`. ADR 0003 → `superseded by
  0005`. Body of each unchanged per the ADR-immutability rule.

### Security

- THREAT_MODEL F-1, F-2, F-3, F-4, F-5, F-13, F-14, F-15
  → mitigated with named code sites and verifying tests.
- F-6, F-7, F-8, F-9, F-10, F-11, F-12, F-16 → status `applies`,
  documented as accepted-risk or post-alpha follow-up. See
  `THREAT_MODEL.md` for per-finding rationale and
  `docs/plan-v0.1.md` §"Post-alpha tracked work" for the
  schedule.
- `cargo audit` reports zero advisories across 158 dependencies.

### Verified

- 67/67 Rust tests pass on Windows under default features.
- 80/80 Rust tests pass with `llamacpp-integration` enabled
  (3 tier-3 stubs skip cleanly when no GGUF is present).
- 2/2 Go tests pass, including the round-trip against the
  spawned daemon binary.
- Workspace clippy `-D warnings` clean in both feature
  configurations.

### Not yet verified

- Real Gemma 4 GGUF run (M2c handle exists; runtime smoke is the
  operator's call).
- CI workflows on real GitHub Actions runners.
- Linux + macOS test execution (Rust toolchain runs only on
  Windows so far).
- External Go consumer importing `clients/go` end-to-end.
