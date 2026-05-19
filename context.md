# context.md — brief for new contributors

This doc tells you *why* inferd exists, *what* it has to do, and *where the authoritative references live*. Read this first before touching any file.

## Why inferd exists

Apps that run local LLMs on a developer machine (CLI tools, IDE assistants, agent runtimes, web apps, middleware) typically embed their own inference engine: their own model loader, their own KV cache, their own sampling loop. Install two such apps and the user is paying for two warm copies of the same multi-GB model — RAM, disk, GPU contention, all duplicated.

inferd fixes this by being the **single local inference endpoint for the whole machine**. Every consumer becomes a thin client. One warm model in memory, many concurrent consumers over a stable IPC contract.

The cost of *not* having inferd is:

- Multi-GB redundant model loads, even when every consumer wants the same model and quantisation.
- Per-app re-implementations of the warm-model lifecycle: load, KV cache, admission queue, graceful shutdown, sandboxing.
- No single place to swap a local backend for a hosted one without every app changing code.

inferd is small, hardened, and standalone. It does not know which apps are connected to it, what those apps want to do with the tokens, or what prompts they intend to send. It is plumbing.

## The architectural shape

```
                                   ┌─→ local model     (llama.cpp FFI today,
                                   │                    GGML/MLX/etc. in scope later)
[any consumer] ←—NDJSON-IPC v1—→ inferd
                                   └─→ remote model    (OpenAI-compat, Anthropic,
                                                        Bedrock, etc. — v0.2)
```

Three things this fixes:

1. **The consumer side is open.** Middleware, CLI tool, IDE plugin, agent runtime, web app — anything that speaks NDJSON-over-IPC v1. We do not pick consumers, compete for them, or gate them.

2. **The model side is the abstraction.** `Backend` trait + router (ADR 0007). v0.1 ships one concrete backend (`llamacpp` local). v0.2 adds remote backends behind the same trait. **The wire surface to consumers is identical regardless of where the model lives.** A consumer cannot tell whether the model is on the same machine or in `us-east-1` from anything except the `backend` field on the `done` frame.

3. **Anything HTTP-shaped lives outside.** Ollama-compat-as-a-server, OpenAI-compat-as-a-server, web UI, REST gateway — all are ecosystem-extension processes, separate repos, separate release cadences. ADR 0006 is load-bearing for that. The daemon never grows an HTTP server, never speaks SSE, never parses request bodies.

The reason this asymmetry exists in concrete terms: if `inferd-daemon` spoke HTTP itself, ADR 0006 would die. The daemon would have content negotiation, request body limits, CORS, OpenAPI schemas, rate-limit headers — all the things "lean core" exists to keep out. And every "compat with X" demand becomes a daemon feature. Keeping HTTP outside the daemon is what keeps the protocol stable enough to freeze (ADR 0008).

### v0.1 → v0.2 — what changes, what doesn't

| | v0.1 | v0.2 |
|---|---|---|
| Consumer wire surface | NDJSON-over-IPC v1 | **identical** |
| `Backend` trait | one concrete (llamacpp) | many concretes (cloud + local) |
| Router | no-op (single backend) | operator-policy across N backends, circuit breakers per ADR 0007 |
| Daemon HTTP | none | none |
| Protocol changes | n/a | none — frozen, additive only |

The load-bearing claim of the architecture is that v0.2's cloud work changes **what's behind the protocol**, not the protocol itself. That's why ADR 0008 froze v1 before any of the cloud adapters existed.

## What inferd has to be

A small, hardened Rust daemon that:

1. Loads a configured backend at startup and keeps it warm.
2. Accepts NDJSON-framed requests on a Unix socket, Windows named pipe, or loopback TCP.
3. Serialises inference through a single active generation + bounded admission queue.
4. Streams tokens back over the same connection. Terminates with one `done` or one `error` frame.
5. Supports multiple backend adapters behind a single `Backend` trait. v0.1 ships local-only; v0.2 adds remote (OpenAI-compat covers vLLM/LM Studio/LocalAI/llama.cpp's HTTP server in addition to OpenAI itself; further adapters added behind the same trait).
6. Enforces per-caller identity (UID on Unix, SID on Windows) and an optional API key for TCP deployments.
7. Stores models in a shared content-addressable layout under `$MODELS_HOME` so multiple tools that adopt the same convention can reuse blobs.

For v0.1, **only the local llama.cpp backend is required** (linked via FFI from a vendored `llama.cpp` submodule). The adapter trait must be designed in from day one so v0.2 can add cloud + remote backends without a rewrite.

## Wire protocol

Protocol v1 is frozen. Authoritative spec: `docs/protocol-v1.md`. Highlights:

- Request framing: one JSON object per line, `\n`-terminated.
- Request fields: `id`, `messages[].{role,content}`, `temperature`, `top_p`, `top_k`, `max_tokens`, `stream`, `grammar` (optional; GBNF constraint passed through to the engine).
- Response framing: NDJSON frames with a `type` discriminator: `token`, `done`, `error`, `status`.
- `done` frames carry `stop_reason` and `backend`; `error` frames carry `code`. See ADR 0008.
- Image-token-budget validation: if a message has image content, the image budget must be in {70, 140, 280, 560, 1120} before the image content.
- 64 MiB per-line frame cap. Bounded reader.

Two endpoints:

- **Inference socket** (`/run/inferd/inference.sock` / `\\.\pipe\inferd-infer` / `127.0.0.1:47321`). Bound only after the backend reports `ready`.
- **Admin socket** (`/run/inferd/admin.sock` / `\\.\pipe\inferd-admin`, mode `0600`). Bound first, on daemon start; pushes lifecycle events for installer GUIs and progress UIs to subscribe to during model fetch and load.

## Invariants you must preserve

These are already-paid-for lessons — do not re-open them:

1. **The daemon has zero knowledge of consumers, prompts, or business logic.** It accepts messages arrays + sampling params, streams tokens back. Nothing else.
2. **Fallback-on-error is the caller's responsibility.** The daemon reports errors cleanly; it does not retry, degrade, or rewrite. ADR 0007.
3. **One active generation + bounded queue.** Default 1 active, 10 queued. `Submit` returns `ErrFull` immediately on overflow. Client disconnect cancels the in-flight job.
4. **Single-instance lock** via `std::fs::File::try_lock` on a daemon-owned lock file. Reject pre-existing symlinks at the lock path (THREAT_MODEL F-2).
5. **Inference socket invisible until backend `ready` fires** (THREAT_MODEL F-13). Admin socket is bound earlier so progress events are visible during bring-up.
6. **No elevation.** Per-user daemon. Unix inference socket `0660` group `inferd-users`. Admin socket `0600`.
7. **NDJSON frame cap.** Per-frame 64 MiB cap (THREAT_MODEL F-5). Bounded reader, not auto-growing buffer.
8. **SHA-256 verification of downloaded models is constant-time** (`subtle::ConstantTimeEq`).
9. **Observability is NDJSON** to `~/.inferd/logs/*.ndjson`, verbosity controlled by `INFERD_LOG=0|1|debug`, 3-generation rotation, secret-pattern redactor at write time.
10. **Every `std::process::Command` is reviewed.** v0.1 has zero subprocess engines (ADR 0005, llama.cpp linked via FFI). Any future `Command` invocation needs justification.
11. **The daemon may make outbound HTTPS only for the narrow purpose carved by ADR 0010**: one URL, one SHA, one file, only during first-boot bootstrap, never after `ready`. No HTTP server, no OpenAI-compat, no registry browsing.

## Where to start

Read, in order:

1. This file (done).
2. `docs/plan-v0.1.md` — crate structure, milestone breakdown, and exact responsibilities of each crate.
3. `docs/protocol-v1.md` — the wire contract.
4. `THREAT_MODEL.md` — every finding L2/L4/L5/L6 applies; the remediations are in code, the *why* is in this doc.
5. `docs/adr/` — every accepted ADR is binding. ADRs 0005, 0006, 0007, 0008, 0009, 0010, 0011 are the load-bearing ones for v0.1.

When you propose a change that crosses crate boundaries, changes the wire contract, changes security posture, or commits the team to a long-lived convention, draft an ADR rather than guessing silently.

## Reference implementation

A hand-written Go client lives at `clients/go/`. It is the canonical example for non-Rust consumers and exercises the full wire surface (inference + admin). When you change the protocol or admin envelope, update both clients in the same PR; cross-language drift is the easiest way to break consumers.

## What NOT to do

- Don't invent a new wire protocol or modify v1. Extensions go to v2 on a separate socket.
- Don't add features beyond the v0.1 scope without an ADR explaining why. The lean-core posture (ADR 0006) is the default.
- Don't introduce async runtime pluralism. Tokio everywhere.
- Don't make the daemon speak HTTP — *any* HTTP, in *any* direction except the ADR 0010 outbound model-fetch carve-out. Inbound HTTP-to-NDJSON adapters (Ollama-compat-as-a-server, OpenAI-compat-as-a-server, web UI) live in separate processes. Outbound NDJSON-to-HTTP adapters (cloud backends) are wrapped behind the `Backend` trait inside the daemon, but the trait abstracts the HTTP away — the rest of the daemon stays HTTP-free.
- Don't embed registry-browsing or model-search in the daemon (ADR 0010). The fetch surface is one URL + one SHA.
- Don't add multi-model warm pooling, ever (ADR 0012). One warm model per inferd process; operators who need N concurrent models run N inferd processes. The router multiplexes backends (local + cloud), not models.
