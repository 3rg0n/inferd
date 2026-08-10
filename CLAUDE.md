# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

**v0.8.0 (prepped on `main`, not yet tagged).** v0.1 through v0.7 have shipped. Seven crates ship: `inferd-proto`, `inferd-engine`, `inferd-daemon`, `inferd-client`, `inferd-openai-wire` (shared OpenAI wire types for the HTTP bridge + outbound adapter), `inferd-http` (a separate, user-launched OpenAI-compat bridge process), and `inferdctl` (the CLI). `wire_version` has not moved since it was introduced: the generation wire is frozen per [ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md), embeddings per [ADR 0017](docs/adr/0017-embeddings-on-a-third-socket.md), rerank per [ADR 0027](docs/adr/0027-reranking-on-a-fourth-socket.md), and every release since v0.4 has been additive on the wire.

What v0.8.0 adds over v0.7.0 — one **behaviour** break, no wire break, so minor rather than patch:

- **`tool_choice` on the v2 wire, enforced by grammar** ([ADR 0029](docs/adr/0029-tool-choice-is-enforced-by-grammar-not-advertised.md), issue #38). `"auto"` / `"required"` / `"none"`, optional and additive. `required` compiles the loaded family's tool-call syntax to GBNF and installs it on the sampler, so *ending the turn* without a call is not a reachable sampling path — not a hint the model may decline. Enforcement hangs off the ADR 0026 renderer registry as `ChatRenderer::tool_call_grammar`, whose **default implementation refuses every mode**: a family opts in deliberately rather than inheriting a silently-unenforced `required`. Gemma 4 is the only opt-in so far (its call syntax is not JSON, so the `json_schema_to_gbnf` path cannot express it — the grammar is hand-written). Rejected with `invalid_request`: `tool_choice` without `tools`, and `tool_choice` alongside `response_format` (only one grammar can be installed, so honouring either silently drops the other). **Scope limit:** call syntax and tool *names* are constrained; argument values are not checked against each tool's `input_schema` — issue #63 tracks that, and upstream llama.cpp carries the same TODO.
- **`tool_choice_unsatisfied` on the `done` frame** (issue #62). `required` bounds where the turn may *end*, not what it contains, so a model that disagrees can emit non-call text until the budget runs out and stop at `max_tokens` — indistinguishable from an ordinary truncation. The flag says which happened. Computed once in the daemon's relay (which already sees whether a `ToolUse` crossed the stream) rather than per-backend, so no adapter can forget the bookkeeping and report "satisfied". `skip_serializing_if`, so it never reaches the wire unless true. `stop_reason` is deliberately unmoved: `StopReasonV2` is a **closed** set on both sides (no catch-all in `inferd-proto`, fixed string constants in `clients/go`), so a new variant would be a parse error on every deployed client rather than a graceful degrade.
- **An unpaired `tool_result` is rejected instead of guessed at** — the breaking change. A `tool_result` whose `tool_call_id` matches no `tool_use` earlier in the same request now fails with `invalid_request`. The Gemma 4 renderer previously inferred the tool name when `tools[]` had exactly one entry and emitted unlabelled content otherwise, so a result could reach the model attributed to a tool that was never called. **Migration:** a caller replaying a tool conversation must include the `tool_use` blocks, not only the `tool_result`s.

What v0.7.0 added: **rerank on a fourth socket** ([ADR 0027](docs/adr/0027-reranking-on-a-fourth-socket.md)) — additive, and a daemon whose model has no classification head binds no rerank socket; and a **second archive per platform** ([ADR 0028](docs/adr/0028-airgapped-build-profile.md)) — the same commit built `--no-default-features` with no HTTPS client linked at all, for hosts that load models via `inferdctl import`. Ten archives across five platforms.

What v0.6 added: vendored **llama.cpp `b9850`** with **Gemma 4 12B**; **boot-time model auto-selection** ([ADR 0023](docs/adr/0023-boot-time-model-auto-selection-by-accelerator-memory.md) — `model_autoselect: "auto"` picks 12B when accelerator total memory ≥ `model_autoselect_min_vram_gib`, default 20 GiB, else E4B); the **`inferd-http` OpenAI-compat bridge** ([ADR 0020](docs/adr/0020-inferd-http-bridge-is-a-separate-process.md)) with vision and structured output; and in v0.6.1 two THREAT_MODEL fixes — **F-1** per-request attachment bounds (`MAX_ATTACHMENTS_PER_REQUEST` 32 / `MAX_ATTACHMENT_BYTES_PER_REQUEST` 128 MiB; the 64 MiB cap bounds one *frame*, and each declared attachment entitled the sender to one more) and **F-17** bounded response writes (`--write-timeout-secs`, default 60s — writes happen downstream of the admission gate, so a peer that stopped reading held a generation slot forever).

What landed across v0.1–v0.5: runtime accelerator detection (ADR 0019 — strongest of Metal / CUDA / ROCm / Vulkan / CPU at boot), the v2 typed-content wire protocol (ADR 0015), the embeddings third socket (ADR 0017), the unified length-prefixed BLOB wire (ADR 0021 — v1 folded into v2 and removed), inbound-TCP removal (ADR 0022), cloud backend adapters (`openai-compat`, `bedrock-invoke`), and the gateway-not-pipe positioning (ADR 0013). Treat `context.md`, the ADRs (`docs/adr/`, especially 0021/0017 for the live wire), and `CHANGELOG.md` as the authoritative spec; `docs/protocol-v2.md` is the normative wire spec and `docs/protocol-v1.md` is a historical record of the removed v1 surface. The workspace version in the root `Cargo.toml` is the source of truth for the current release. Releases ship on five platforms — Linux x86_64 (CUDA), Linux arm64, macOS arm64 (Metal), Windows x86_64 (CUDA), Windows arm64 — with install=work validated on the three desktop platforms per release (`docs/vX.Y-validation.md`). **Windows arm64** was parked at v0.6.0 for a b9850 OpenMP load crash, fixed by `GGML_OPENMP=OFF` on arm64 (ggml self-threads via its own pthread pool; verified on real arm64 hardware) and shipping since. Do not "re-fix" this by staging `libomp.dll`: the missing import was `libomp140.aarch64.dll`, and two DLL-staging attempts failed before `dumpbin /dependents` on an actual arm64 runner settled it.

Before writing code, read `context.md` — it is the hand-off brief for new contributors and names the non-negotiable invariants the daemon must preserve.

Releasable means **install=work**: a fresh-machine installer → real `generate` + real `embed`, no mock backend, no hand-edited config, no "run pull first." Mock-default install scripts are a release blocker. Don't cut a release tag until end-to-end works on the target platform; confirm with the user before any `git tag` / `gh release create` / `cargo publish`.

## What inferd is

inferd is a single host-wide local-inference daemon. One warm model in memory, many consumers over IPC (length-prefixed frames for generation, NDJSON for embeddings). It is plumbing, not a product — anything on the machine that wants local inference (CLI tools, IDE assistants, agent runtimes, web apps, middleware) connects to inferd instead of bundling its own engine.

Reference consumer projects exist as examples, but inferd does not encode any consumer's assumptions. The daemon's only contract is the wire protocol.

## Wire protocol is frozen — one generation surface + embeddings

Each wire surface is frozen the moment it ships. As of v0.4 ([ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)) breaking changes to the generation wire bump the in-band `wire_version` (the daemon fails loudly on a mismatch with `wire_version_unsupported`) rather than spawning a new socket — this supersedes the older "successor on a separate socket path, no in-band negotiation" stance for the generation surface. Backwards-additive changes (new optional fields older servers MUST ignore and older clients MUST NOT require) stay acceptable; parsers enforce "unknown fields ignored on parse" so the door for additive changes stays open. Frame cap: 64 MiB per frame, explicit byte limit (enforced on the length prefix before the payload is read) — use a bounded reader, not an auto-growing buffer.

Two live wire surfaces, each on its own socket and each independently frozen:

| Surface | Socket (Linux) | Framing | Shape | ADR |
|---|---|---|---|---|
| generation (v2) | `inferd.sock` (Win `\\.\pipe\inferd`) | length-prefixed, type-tagged (`[uvarint len][1 byte type: 0x01 JSON / 0x02 BLOB][payload]`) | typed content blocks, attachments (raw BLOB frames keyed by `attachment_id`), tools, thinking; in-band `wire_version` | [0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md) (supersedes the framing/socket parts of 0008/0009/0015) |
| embeddings | `infer.embed.sock` | NDJSON | single-frame `embeddings` response, MRL `dimensions`, `task` prefix | [0017](docs/adr/0017-embeddings-on-a-third-socket.md) |

v1 was the original text-only generation surface; v0.4 folded it into v2 (a text-only request is a single `text` content block) and removed the v1 socket and types. A socket is bound **only when** the active backend advertises that capability (`supports()`): an embedding-only model binds the embed socket and not generation; a generation-only model binds the generation socket and not embed. The admin socket is shared across all surfaces. The generation and embed shapes are specified in ADRs 0021/0015/0017; `docs/protocol-v1.md` is retained as a historical record of the removed v1 surface.

## Model store

Models live in a shared content-addressable store at `$MODELS_HOME` per [ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md). Resolution order:

1. `models_home` field in `~/.inferd/config.json`.
2. `MODELS_HOME` env var.
3. Platform default: `${XDG_DATA_HOME:-$HOME/.local/share}/models/` (Linux), `~/Library/Application Support/models/` (macOS), `%LOCALAPPDATA%\models\` (Windows).

Layout: `blobs/sha256/<aa>/<full-hash>/data` for blobs, `manifests/<name>.json` for the name-to-blob map, `locks/<name>.lock` for advisory writer locks. Producers stream into `blobs/sha256/<aa>/.partial-<hash>/data.tmp`, verify SHA, atomic-rename, then write the manifest.

The store is wire-compatible with the cross-tool *Shared Local Model Store* convention so other tools that adopt the same shape can reuse blobs inferd has fetched and vice versa.

## Model reference material

`docs/` contains upstream Gemma 4 reference docs — not inferd design, but context on the model the default `llamacpp` backend serves:

- `run-gemma-content-generation-and-inferences.md` — framework/variant landscape
- `text.function.calling.with.gemma.4.md` — tool-use schema (now expressed on the v2 wire as typed tool blocks, ADR 0015)
- `thinking.mode.in.gemma.md` — reasoning trace separation (now a v2 response-frame concern, ADR 0015)

Treat these as background, not requirements. Tool-use and thinking semantics live on the v2 generation wire (typed content blocks; text-only is a single `text` block). The daemon owns the model-specific shaping that turns semantic v2 frames into Gemma's wire format (ADR 0013 — gateway, not pipe).

## Architecture

Single `cargo workspace` at repo root. Crates:

| Crate | Bin/lib | Role |
|---|---|---|
| `inferd-proto` | lib | Wire format for both live surfaces: `v2/` (typed content blocks — the single generation surface), `embed/` (embeddings), `error.rs`, `frame.rs` (length-prefixed type-tagged codec for generation + NDJSON read/write for embed, 64 MiB cap). The v1 `request.rs`/`response.rs` modules were removed in v0.4. `no_std`-friendly. |
| `inferd-engine` | lib | `Backend` trait (`backend.rs`) + adapters: `llamacpp/` (FFI to vendored `libllama` via `ffi.rs` + `mtmd_ffi.rs` for multimodal), `mock.rs` (tests), `openai_compat/` and `bedrock_invoke/` (outbound-HTTPS cloud adapters, feature-gated). |
| `inferd-daemon` | bin `inferd-daemon` | Lifecycle (`lifecycle.rs` / `lifecycle_v2.rs` / `lifecycle_embed.rs` — one per wire surface), admission `queue.rs`, single-instance `lock.rs`, `endpoint.rs` (UDS / named pipe — inbound TCP removed in v0.5.0, ADR 0022), `admin.rs`, `peercred.rs` + `windows_security.rs` (pipe DACL), `router.rs`, `autoselect.rs` (ADR 0023 boot-time model pick), CAS `store.rs`, `fetch.rs`, activity `logx.rs` + `redact.rs`. IPC-only — links no HTTP server, and carries no shared-key auth module: every surviving transport is authenticated by kernel peer credentials. |
| `inferd-client` | lib | Rust client: `v2_client.rs` (the generation surface), `embed_client.rs`, the shared `ClientError` in `client.rs`, plus `admin.rs` subscriber and `wait.rs` connect-and-retry. The v1 `Client` was removed in v0.4. Published to crates.io. |
| `inferd-openai-wire` | lib | The OpenAI Chat/Embeddings wire structs (both `Serialize` + `Deserialize`), shared by the outbound `openai_compat` adapter (in `inferd-engine`, feature-gated) and the inbound `inferd-http` bridge so the two directions cannot drift. `MessageContent` (string-or-parts), `ResponseFormat`, etc. Dependency-light (serde only). |
| `inferd-http` | bin `inferd-http` | **Separate, user-launched** OpenAI-compat HTTP bridge (ADR 0020) — NOT part of the daemon. Exposes `/v1/chat/completions` (stream + non-stream), `/v1/embeddings` (float + base64), `/v1/models`, `/health`; translates them to the daemon's v2/embed IPC via `inferd-client`. Supports vision (`image_url` → decoded RGB attachment), **audio** (`input_audio` → decoded + **resampled** mono LE-f32 PCM, ADR 0025) and structured output (`response_format` json_schema → grammar). A consumer, not a privileged surface (ADR 0014). The only crate linking MPL-2.0 code (`symphonia`); `deny.toml` enforces that containment. |
| `inferdctl` | bin `inferdctl` | Single CLI in the gh / kubectl shape (renamed from `inferd` per [ADR 0018](docs/adr/0018-cli-renamed-to-inferdctl.md)). Subcommands: `status`, `watch`, `pull`, `doctor`. Crate dir is `crates/inferd/` but the package and binary are `inferdctl`. |

The `inferdctl` CLI is a **reference middleware, not a privileged surface** ([ADR 0014](docs/adr/0014-inferd-cli-is-a-reference-middleware.md)) — it talks to the daemon over the same `inferd-client` library every other consumer uses. Cloud adapters live behind cargo features (`openai`, `bedrock`); the dynamic-loader accelerator path lives behind `dl-backends` (ADR 0019). `cargo tree -e features` is the verifiable boundary for what a given build links.

Clients (`clients/go/`, `clients/py/`, `clients/ts/`) are hand-written wrappers shipped alongside the daemon. The Go client is the canonical example for non-Rust consumers — **`clients/py/` and `clients/ts/` are README-only stubs**, not implementations.

Layout gotchas that don't match their names:

- `crates/inferd/` builds the package **`inferdctl`** (ADR 0018 renamed the CLI; the directory did not follow).
- `clients/go/` has its own `go.mod`, so Go treats it as a nested module needing **path-prefixed tags** (`clients/go/vX.Y.Z`) — `release.yml` pushes these automatically; a plain `vX.Y.Z` tag alone makes `go get` fail with "unknown revision".
- `vendor/llama.cpp` is a submodule; `crates/inferd-proto/fuzz` is deliberately excluded from the workspace.
- `packaging/` holds the real installers (`windows/install.ps1`, `systemd/`, `launchd/`) — these are what "install=work" exercises.
- `docs/vX.Y-validation.md` records the per-platform install=work matrix for each release; add a row rather than starting a new format.

### Flow at runtime

1. Daemon boots, acquires single-instance lock (flock on Unix, LockFileEx on Windows). Admin socket is bound *immediately* so progress UIs can connect during the rest of bring-up.
2. Daemon reads `~/.inferd/config.json`, opens the model store, resolves or fetches the configured model into the CAS layout, verifies SHA-256 with constant-time compare.
3. Backend reports ready (for the llama.cpp FFI backend: model load + KV-cache allocation succeed; with `dl-backends`, this is also where `ggml_backend_load_all()` runs and the strongest available accelerator is selected per the ADR 0019 cascade). **Only then** does the daemon bind the inference socket(s) the backend's capabilities advertise — never before.
4. Client connects to the matching surface socket and sends frames — length-prefixed type-tagged frames for generation (ADR 0021), NDJSON for embed.
5. Admission queue (default: 1 active generation, 10 queued). Overflow returns `{"type":"error","code":"queue_full",...}` immediately.
6. Router picks a backend per ADR 0007 operator policy. Backend streams tokens back as `{"type":"token",...}` frames, terminates with one `done` or one `error` (the `done`/terminal frame carries the `backend` field per ADR 0009). Embed requests return a single terminal `embeddings` frame instead.
7. Client disconnect cancels the in-flight job. No retry/fallback in the daemon, no mid-stream failover — the caller owns retry.

## Non-negotiable invariants (from `context.md` §"Invariants")

When extending or refactoring, these are already-paid-for lessons — do not re-open:

1. Daemon has **zero knowledge** of middlewares, processors, or prompts. Messages array + sampling params in; tokens out.
2. Fallback on error is the **caller's** responsibility. Daemon reports cleanly; no retry/degrade/rewrite.
3. One active generation, bounded queue. `ErrFull` is non-blocking.
4. Single-instance lock file must **reject pre-existing symlinks** (THREAT_MODEL F-2).
5. Inference socket invisible until backend `ready` fires (THREAT_MODEL F-13). Admin socket is bound earlier so progress events are visible during bring-up.
6. No elevation. Per-user daemon. Unix socket `0660`, group `inferd-users`. Admin socket `0600`.
7. 64 MiB frame cap, explicit byte limit (THREAT_MODEL F-5) — enforced on the length prefix before the payload on generation, on the line length for embed NDJSON.
8. SHA-256 verification of downloaded models uses **constant-time** compare (`subtle` crate).
9. Activity log NDJSON → `~/.inferd/logs/*.ndjson`, `INFERD_LOG=0|1|debug`, 3-generation rotation, secret redactor at write time.
10. Every `std::process::Command` is reviewed. inferd has **no subprocess engines** (per ADR 0005, llama.cpp is linked via FFI; ADR 0019's dynamic-loader path stays in-process via `dlopen`, still not a subprocess). Any `Command` invocation is a code smell needing justification.
11. The daemon may make outbound HTTPS only for the narrow purpose carved by [ADR 0010](docs/adr/0010-narrow-https-exception-for-model-bootstrap.md): one URL, one SHA, one file. No HTTP server, no OpenAI-compat, no registry browsing, no HTTP after `ready`.
12. **No image or audio codec in the daemon** (ADR 0016). The consumer decodes and — since v0.6.2 — owns *rate conversion* too: an `Attachment::Audio`'s `sample_rate` MUST equal the backend's advertised `audio_sample_rate` or the daemon rejects it. It never resamples, because libmtmd's audio entry point takes no rate argument, so the wrong rate is a fluent wrong answer rather than a detectable error. `inferd-http` is the reference consumer that decodes + resamples (ADR 0025) and is the **only** crate permitted to link MPL-2.0 `symphonia` — `deny.toml` fails the build if that reaches the daemon, the engine, or either published library.

[ADR 0005](docs/adr/0005-libllama-ffi-not-subprocess.md) (supersedes 0003): the v0.1 default backend is `libllama` linked via FFI from a vendored `llama.cpp` submodule. No subprocess llamafile. No HTTP server compiled into the daemon. The `Backend` trait stays the same; only the default adapter changes.

[ADR 0006](docs/adr/0006-lean-core-ecosystem-extensions.md): lean-core posture. The daemon ships IPC (length-prefixed frames for generation, NDJSON for embed) + `Backend` trait + admission queue + router + security perimeter. **HTTP, OpenAI-compat, web UI, gRPC are not in the daemon** — they live as separate processes that talk to inferd over that IPC (`inferd-http` is exactly this). Apps do not override the backend on the wire; if an app wants per-call control, it writes its own provider SDK integration.

[ADR 0007](docs/adr/0007-backend-routing-and-failure-semantics.md): routing is operator-configured policy across registered backends. **No in-daemon retry. No mid-stream failover.** Circuit breaker is the only stateful policy mechanism. v0.1 router is a no-op (one backend); v0.2 adds cloud adapters + real policy.

[ADR 0009](docs/adr/0009-pre-m1-open-questions-resolved.md): admin socket is a separate endpoint (`0600` on Unix), peer credentials enforced on UDS + named pipe, backend identity exposed via `backend` field on `done` frames. (Two clauses are superseded: "separate-socket-per-version, no in-band negotiation" by ADR 0021 — the generation wire now carries an in-band `wire_version`; and "TCP gets API key only" by ADR 0022 — inbound TCP and the shared-key path were removed in v0.5.0, so peer credentials are the *only* authentication.)

[ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md): models live in a shared CAS store at `$MODELS_HOME`. Manifest indirection (`name → sha`) plus content-addressed blob paths. Wire-compatible with the cross-tool convention.

[ADR 0013](docs/adr/0013-inferd-is-the-gateway-not-the-pipe.md): inferd is a **gateway, not a pipe**. The daemon owns model-specific shaping — chat templating, attachment routing (mtmd for llamacpp), tool-call lifecycle, embed task-prefixes and MRL truncation. Consumers send *semantic intent* (`messages[]`, `attachments[]`, `tools[]`); the daemon translates to what the engine consumes. This is distinct from ADR 0006 (which is about consumer-facing surfaces like HTTP/web UI staying out); engine-level shaping is squarely a daemon concern. Invariant #1 ("zero knowledge of prompts") was the original v1 text-only framing; on the v2 generation wire the daemon knows the *engine's* format, never the *consumer's* application logic.

## Scope gates (what NOT to build)

- No HTTP/gRPC transport in the daemon — ever (ADR 0006). IPC only. HTTP is an ecosystem-extension job, separate process. The narrow ADR 0010 HTTPS exception is for model bootstrap only and explicitly forbids serving HTTP.
- No OpenAI-compat surface in the daemon — ever (ADR 0006). Same pattern.
- No per-request backend override on the wire — ever (ADR 0006, ADR 0007). Apps wanting per-call provider control should write their own SDK integration.
- No in-daemon retry on backend failure (ADR 0007). Caller owns retry.
- No mid-stream failover, ever (ADR 0007). Structurally broken; explicitly rejected.
- No subprocess engines, ever (ADR 0005). llama.cpp is linked via FFI / `dlopen`, never spawned.
- No multi-model warm pool, ever ([ADR 0012](docs/adr/0012-one-warm-model-per-inferd-process.md)). One warm model per inferd process; operators who need N concurrent models (or gen + embed) run N inferd processes. The router (ADR 0007) multiplexes *backends*, not *models*.
- No registry browsing, model search, or arbitrary HTTP fetches in the daemon (ADR 0010). The fetch surface is one URL + one SHA.
- No breaking changes to any shipped wire surface. The generation (v2) and embed surfaces are each frozen (ADR 0021 / 0017). Additive optional fields only; a breaking change to the generation wire bumps the in-band `wire_version` (ADR 0021), embed-breaking changes go to a successor socket.
- No async runtime pluralism. Tokio everywhere.
- No NPU paths in the accelerator cascade (ADR 0019). Metal / CUDA / ROCm / Vulkan / CPU only; revisit NPUs when vendor toolchains beat CPU+SIMD on LLM decode.
- No mid-stream backend switching or multi-GPU sharding (ADR 0019). Pick one accelerator at boot; pick one backend per request.

## Commands

### First clone

`vendor/llama.cpp` is a submodule and the `llamacpp` feature will not build without it:

```sh
git submodule update --init --recursive
```

Building any `llamacpp`-derived feature also needs CMake + a C++ toolchain and libclang for bindgen: `libclang-dev` on Ubuntu, `brew install llvm` on macOS. On Windows, the CUDA and arm64 paths drive CMake with the **Ninja** generator (not MSBuild), so `cl.exe` and `ninja` must be on `PATH` — run from an MSVC dev shell, or the configure step fails.

### Pre-commit gate

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo audit
cargo deny check licenses bans sources
```

Run the full lint + test + audit cycle across the whole workspace before every commit, not just changed files.

`deny.toml` landed with ADR 0025 and gates two things: the licence allow-list, and that MPL-2.0 `symphonia` stays reachable **only** from `inferd-http`. Unlike `cargo audit` (schedule-only, non-blocking — hard-blocking PRs on a moving vuln DB is the posture that got Trivy compromised), the licence check is deterministic and *does* block PRs via the `licenses` CI job. Adding a copyleft dependency is a licence-posture decision: write the ADR, don't just extend the exception list. Advisory gating stays with `cargo audit`, which is why the CI job runs `check licenses bans sources` and not a bare `check`.

**`--all-targets` is not optional.** Omitting it skips test/bench code, where clippy findings have slipped through and failed every CI job after the fact. And `--all-features` alone does not reproduce CI: `ci.yml` gates on a **per-feature matrix**, not one union build, because the GPU features are mutually exclusive in practice. Before pushing anything that touches `inferd-proto` types, `inferd-engine` adapters, or daemon wiring, run the variants CI runs:

```sh
cargo clippy --all-targets -- -D warnings                                  # default (mock)
cargo clippy --all-targets --features inferd-engine/openai -- -D warnings
cargo clippy --all-targets --features inferd-engine/llamacpp -- -D warnings
cargo clippy --all-targets --features inferd-engine/dl-backends -- -D warnings
cargo test -p inferd-daemon --features security --test security            # Tier 5
```

Adding a field to `ResolvedV2` / `RequestV2` is the recurring trap: struct literals in the `openai`, `bedrock`, and `security` feature-gated code break, and plain `cargo test --all` does **not** compile them.

### Test tiers (`docs/test-strategy.md`)

| Tier | What | How |
|---|---|---|
| 1–2 | unit + daemon integration on the `mock` backend | `cargo test --all` (default features) |
| 3 | engine against real `libllama` | `--features inferd-engine/llamacpp-integration` + `INFERD_TEST_MODEL_PATH` (and `INFERD_TEST_EMBED_MODEL_PATH`); **skips silently** without them |
| 4 | cross-language wire validation | `cd clients/go && go vet ./... && go test ./...` |
| 5 | security regressions | `cargo test -p inferd-daemon --features security --test security` |
| 6 | fuzzing | `crates/inferd-proto/fuzz` — deliberately **not** a workspace member (needs nightly) |

Single test: `cargo test -p <crate> <test_name>`. Fast loops: `cargo test -p inferd-proto` (<5 s), `cargo test -p inferd-daemon` (<30 s).

Tier 3 is the only tier that catches real-model regressions. Mock-backend tests have twice passed while all real generation was broken — if you touch prefill, templating, capabilities, or the FFI adapter, Tier 3 is mandatory, not optional.

### Building and running with a backend

```sh
cargo build -p inferd-daemon --features dl-backends   # runtime accelerator pick (ADR 0019)
cargo build -p inferd-daemon --features cuda          # static single-accelerator path
```

`INFERD_FORCE_BACKEND=cpu|metal|cuda|rocm|vulkan` pins the accelerator at runtime (env-only, no CLI flag — ADR 0019). `INFERD_LOG=0|1|debug` controls the activity log.

With `dl-backends`, `build.rs` stages the ggml backend shared libs into `target/<profile>/backends/` and release packaging picks them up from there; the install scripts flatten that directory next to the binary. If the daemon reports the CPU backend on a GPU box, a missing/stale lib in `backends/` is the first thing to check.

### Cutting a version bump

The workspace version in the root `Cargo.toml` is the source of truth, but **10 internal `=X.Y.Z` path-dep pins do not inherit it**. Bump all of them plus the lockfile or the build breaks late:

```sh
grep -rn 'version = "=' --include=Cargo.toml crates/   # find all 10
cargo update -w                                        # refresh Cargo.lock
```

`docs/RELEASING.md` is the full tag/publish runbook. Release is tag-triggered (`vX.Y.Z`) across 5 platforms with `fail-fast: false`; `publish` needs every build leg, so one red platform publishes nothing. Only `inferd-proto` and `inferd-client` go to crates.io, and **the workflow does not publish them** — that is a deliberate manual step (`docs/RELEASING.md` §5). A `crates-io` job existed through v0.6.1 and was deleted because it no-op'd to *success* without the token, making a green run look like the crates had shipped.

Feature-gated builds and tests:

- The daemon and engine pick adapters at compile time via cargo features. Key ones: `llamacpp` (FFI backend), `dl-backends` (ADR 0019 dynamic-loader / runtime accelerator selection — implies `llamacpp`), `cuda` / `metal` / `vulkan` / `rocm` (per-backend opt-ins), `openai` / `bedrock` (cloud adapters), `security` (Tier-5 regression tests in the daemon).
- Integration tests that need a real llama.cpp build are gated behind `llamacpp-integration`; set `INFERD_TEST_MODEL_PATH` to a GGUF file to actually run them, otherwise they skip.
- Run a single test with `cargo test -p <crate> <test_name>`.
- Build with a backend, e.g. `cargo build -p inferd-daemon --features dl-backends` (or `--features cuda` for the static single-accelerator path).
- `INFERD_FORCE_BACKEND=cpu|metal|cuda|rocm|vulkan` pins the accelerator at runtime (env-only, no CLI flag — ADR 0019).

Release engineering: `docs/RELEASING.md` is the tag/publish runbook; `docs/v0.3-validation.md` is the per-platform install=work coverage matrix. The release workflow (`.github/workflows/release.yml`) bundles backend dylibs into each tarball; the Linux x86_64 CUDA path uses `readelf -d` BFS to walk the transitive DT_NEEDED closure and patchelf to bake `$ORIGIN` RUNPATH (NVIDIA driver libs `libcuda.so.1` / `libnvidia-ml.so.1` are skiplisted — redistribution is EULA-forbidden).

## When writing ADRs

- Location: `docs/adr/NNNN-kebab-case-title.md`, sequential.
- ADRs are **immutable** once accepted. Supersede by writing a new one; set the old `Status:` to `superseded by NNNN`. Update `docs/adr/README.md` index.
- Required when: a decision crosses crate boundaries, changes the wire contract, changes security posture, picks a foundational dependency, or commits the team to a long-lived convention. Skip for local choices.
