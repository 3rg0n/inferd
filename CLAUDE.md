# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

**v0.3.0-rc.** v0.1 and v0.2 have shipped. The current line is v0.3 (branch `v0.3-dev`), which implements runtime accelerator detection per [ADR 0019](docs/adr/0019-runtime-accelerator-detection-via-ggml-backend-dl.md) — the daemon picks the strongest available compute backend (Metal / CUDA / ROCm / Vulkan / CPU) at boot instead of being baked in at compile time. All five crates ship: `inferd-proto`, `inferd-engine`, `inferd-daemon`, `inferd-client`, and `inferdctl`.

What's landed since v0.1: the v2 typed-content wire protocol (ADR 0015), the embeddings third socket (ADR 0017), cloud backend adapters (`openai-compat`, `bedrock-invoke`), and the gateway-not-pipe positioning (ADR 0013). Treat `context.md`, `docs/protocol-v1.md`, `docs/adr/`, and `CHANGELOG.md` as the authoritative spec. The workspace version in the root `Cargo.toml` is the source of truth for the current release.

Before writing code, read `context.md` — it is the hand-off brief for new contributors and names the non-negotiable invariants the daemon must preserve.

Releasable means **install=work**: a fresh-machine installer → real `generate` + real `embed`, no mock backend, no hand-edited config, no "run pull first." Mock-default install scripts are a release blocker. Don't cut a release tag until end-to-end works on the target platform; confirm with the user before any `git tag` / `gh release create` / `cargo publish`.

## What inferd is

inferd is a single host-wide local-inference daemon. One warm model in memory, many consumers over NDJSON-over-IPC. It is plumbing, not a product — anything on the machine that wants local inference (CLI tools, IDE assistants, agent runtimes, web apps, middleware) connects to inferd instead of bundling its own engine.

Reference consumer projects exist as examples, but inferd does not encode any consumer's assumptions. The daemon's only contract is the wire protocol.

## Wire protocol is frozen — and there are now three of them

Each wire surface is frozen the moment it ships. Breaking changes never mutate an existing surface; they go to a successor on a **separate socket path** — no in-band version negotiation. Backwards-additive changes within a surface (new optional fields older servers MUST ignore and older clients MUST NOT require) are acceptable; parsers enforce "unknown fields ignored on parse" so the door for additive changes stays open. Frame cap: 64 MiB per line, explicit byte limit — use a bounded reader, not an auto-growing buffer.

Three live wire surfaces, each on its own socket and each independently frozen:

| Surface | Socket (Linux) | Shape | ADR |
|---|---|---|---|
| v1 generation | `infer.sock` | text-only `Message { role, content: String }`, token stream | [0008](docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md) (supersedes 0001) |
| v2 generation | `infer.v2.sock` | typed content blocks, attachments, tools, thinking | [0015](docs/adr/0015-v2-wire-protocol-typed-content-blocks.md) |
| embeddings | `infer.embed.sock` | single-frame `embeddings` response, MRL `dimensions`, `task` prefix | [0017](docs/adr/0017-embeddings-on-a-third-socket.md) |

A socket is bound **only when** the active backend advertises that capability (`supports()`): an embedding-only model binds the embed socket and not v1/v2; a generation-only model binds v1/v2 and not embed. The admin socket is shared across all surfaces. `docs/protocol-v1.md` documents v1; the v2 and embed shapes are specified in ADRs 0015/0017.

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

Treat these as background, not requirements. v1 stays text-only and frozen per ADR 0008; tool-use and thinking semantics live on the v2 socket. The daemon owns the model-specific shaping that turns semantic v2 frames into Gemma's wire format (ADR 0013 — gateway, not pipe).

## Architecture

Single `cargo workspace` at repo root. Crates:

| Crate | Bin/lib | Role |
|---|---|---|
| `inferd-proto` | lib | Wire format for all three surfaces: `request.rs`/`response.rs`/`error.rs` (v1), `v2/` (typed content blocks), `embed/` (embeddings), `frame.rs` (NDJSON read/write, 64 MiB cap). `no_std`-friendly. |
| `inferd-engine` | lib | `Backend` trait (`backend.rs`) + adapters: `llamacpp/` (FFI to vendored `libllama` via `ffi.rs` + `mtmd_ffi.rs` for multimodal), `mock.rs` (tests), `openai_compat/` and `bedrock_invoke/` (outbound-HTTPS cloud adapters, feature-gated). |
| `inferd-daemon` | bin `inferd-daemon` | Lifecycle (`lifecycle.rs` / `lifecycle_v2.rs` / `lifecycle_embed.rs` — one per wire surface), admission `queue.rs`, single-instance `lock.rs`, `endpoint.rs` (UDS / named pipe / loopback TCP), `admin.rs`, `peercred.rs`, `auth.rs`, `router.rs`, CAS `store.rs`, `fetch.rs`, activity `logx.rs` + `redact.rs`. |
| `inferd-client` | lib | Rust client, one surface each: `client.rs` (v1), `v2_client.rs`, `embed_client.rs`, plus `admin.rs` subscriber and `wait.rs` connect-and-retry. Published to crates.io. |
| `inferdctl` | bin `inferdctl` | Single CLI in the gh / kubectl shape (renamed from `inferd` per [ADR 0018](docs/adr/0018-cli-renamed-to-inferdctl.md)). Subcommands: `status`, `watch`, `pull`, `doctor`. Crate dir is `crates/inferd/` but the package and binary are `inferdctl`. |

The `inferdctl` CLI is a **reference middleware, not a privileged surface** ([ADR 0014](docs/adr/0014-inferd-cli-is-a-reference-middleware.md)) — it talks to the daemon over the same `inferd-client` library every other consumer uses. Cloud adapters live behind cargo features (`openai`, `bedrock`); the dynamic-loader accelerator path lives behind `dl-backends` (ADR 0019). `cargo tree -e features` is the verifiable boundary for what a given build links.

Clients (`clients/go/`, `clients/py/`, `clients/ts/`) are hand-written wrappers shipped alongside the daemon. The Go client is the canonical example for non-Rust consumers.

### Flow at runtime

1. Daemon boots, acquires single-instance lock (flock on Unix, LockFileEx on Windows). Admin socket is bound *immediately* so progress UIs can connect during the rest of bring-up.
2. Daemon reads `~/.inferd/config.json`, opens the model store, resolves or fetches the configured model into the CAS layout, verifies SHA-256 with constant-time compare.
3. Backend reports ready (for the llama.cpp FFI backend: model load + KV-cache allocation succeed; with `dl-backends`, this is also where `ggml_backend_load_all()` runs and the strongest available accelerator is selected per the ADR 0019 cascade). **Only then** does the daemon bind the inference socket(s) the backend's capabilities advertise — never before.
4. Client connects over NDJSON to the matching surface socket, sends frames.
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
7. 64 MiB NDJSON frame cap, explicit byte limit (THREAT_MODEL F-5).
8. SHA-256 verification of downloaded models uses **constant-time** compare (`subtle` crate).
9. Activity log NDJSON → `~/.inferd/logs/*.ndjson`, `INFERD_LOG=0|1|debug`, 3-generation rotation, secret redactor at write time.
10. Every `std::process::Command` is reviewed. inferd has **no subprocess engines** (per ADR 0005, llama.cpp is linked via FFI; ADR 0019's dynamic-loader path stays in-process via `dlopen`, still not a subprocess). Any `Command` invocation is a code smell needing justification.
11. The daemon may make outbound HTTPS only for the narrow purpose carved by [ADR 0010](docs/adr/0010-narrow-https-exception-for-model-bootstrap.md): one URL, one SHA, one file. No HTTP server, no OpenAI-compat, no registry browsing, no HTTP after `ready`.

[ADR 0005](docs/adr/0005-libllama-ffi-not-subprocess.md) (supersedes 0003): the v0.1 default backend is `libllama` linked via FFI from a vendored `llama.cpp` submodule. No subprocess llamafile. No HTTP server compiled into the daemon. The `Backend` trait stays the same; only the default adapter changes.

[ADR 0006](docs/adr/0006-lean-core-ecosystem-extensions.md): lean-core posture. The daemon ships NDJSON-over-IPC + `Backend` trait + admission queue + router + security perimeter. **HTTP, OpenAI-compat, web UI, gRPC are not in the daemon** — they live as separate processes that talk NDJSON to inferd. Apps do not override the backend on the wire; if an app wants per-call control, it writes its own provider SDK integration.

[ADR 0007](docs/adr/0007-backend-routing-and-failure-semantics.md): routing is operator-configured policy across registered backends. **No in-daemon retry. No mid-stream failover.** Circuit breaker is the only stateful policy mechanism. v0.1 router is a no-op (one backend); v0.2 adds cloud adapters + real policy.

[ADR 0009](docs/adr/0009-pre-m1-open-questions-resolved.md): admin socket is a separate endpoint (`0600` on Unix), peer credentials enforced on UDS + named pipe (TCP gets API key only), protocol versioning is separate-socket-per-version (no in-band negotiation), backend identity exposed via `backend` field on `done` frames.

[ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md): models live in a shared CAS store at `$MODELS_HOME`. Manifest indirection (`name → sha`) plus content-addressed blob paths. Wire-compatible with the cross-tool convention.

[ADR 0013](docs/adr/0013-inferd-is-the-gateway-not-the-pipe.md): inferd is a **gateway, not a pipe**. The daemon owns model-specific shaping — chat templating, attachment routing (mtmd for llamacpp), tool-call lifecycle, embed task-prefixes and MRL truncation. Consumers send *semantic intent* (`messages[]`, `attachments[]`, `tools[]`); the daemon translates to what the engine consumes. This is distinct from ADR 0006 (which is about consumer-facing surfaces like HTTP/web UI staying out); engine-level shaping is squarely a daemon concern. Invariant #1 ("zero knowledge of prompts") is the v1 text-only framing; from v2 on, the daemon knows the *engine's* format, never the *consumer's* application logic.

## Scope gates (what NOT to build)

- No HTTP/gRPC transport in the daemon — ever (ADR 0006). IPC only. HTTP is an ecosystem-extension job, separate process. The narrow ADR 0010 HTTPS exception is for model bootstrap only and explicitly forbids serving HTTP.
- No OpenAI-compat surface in the daemon — ever (ADR 0006). Same pattern.
- No per-request backend override on the wire — ever (ADR 0006, ADR 0007). Apps wanting per-call provider control should write their own SDK integration.
- No in-daemon retry on backend failure (ADR 0007). Caller owns retry.
- No mid-stream failover, ever (ADR 0007). Structurally broken; explicitly rejected.
- No subprocess engines, ever (ADR 0005). llama.cpp is linked via FFI / `dlopen`, never spawned.
- No multi-model warm pool, ever ([ADR 0012](docs/adr/0012-one-warm-model-per-inferd-process.md)). One warm model per inferd process; operators who need N concurrent models (or gen + embed) run N inferd processes. The router (ADR 0007) multiplexes *backends*, not *models*.
- No registry browsing, model search, or arbitrary HTTP fetches in the daemon (ADR 0010). The fetch surface is one URL + one SHA.
- No breaking changes to any shipped wire surface. v1 / v2 / embed are each frozen (ADR 0008 / 0015 / 0017). Additive optional fields only; breaking changes go to a successor socket.
- No async runtime pluralism. Tokio everywhere.
- No NPU paths in the accelerator cascade (ADR 0019). Metal / CUDA / ROCm / Vulkan / CPU only; revisit NPUs when vendor toolchains beat CPU+SIMD on LLM decode.
- No mid-stream backend switching or multi-GPU sharding (ADR 0019). Pick one accelerator at boot; pick one backend per request.

## Commands

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo audit
cargo deny check
```

Run the full lint + test + audit cycle across the whole workspace before every commit, not just changed files.

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
