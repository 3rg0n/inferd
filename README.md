# inferd

> Local inference daemon. One warm model, many consumers.

**Status: alpha.** Code is in flight; v0.1 is shipping toward GA. See
`docs/plan-v0.1.md` for the design and `context.md` for the hand-off
brief to first-time contributors.

inferd is a single host-wide Rust service that owns the hard parts of
running a local LLM — loading the model, holding it warm, multiplexing
requests, swapping backends — so that every app on the machine shares
one daemon instead of spawning its own.

## Why

Every app that embeds its own inference engine burns RAM and CPU
duplicating the same warm model. One developer laptop running two
such apps = two copies of a multi-GB model, both busy holding tokens.
The same shape applies on a server: every web app that talks to a
local LLM ends up reinventing the same warm-model lifecycle.

inferd solves that by being the *only* local inference endpoint on the
host. It:

- Loads a model once, keeps it warm.
- Exposes a small NDJSON-over-IPC protocol on a Unix socket, Windows
  named pipe, or loopback TCP.
- Enforces per-caller identity (UID on Unix, SID on Windows) and an
  optional API key for loopback TCP.
- Multiplexes requests through a single engine or across a pool of
  backend adapters.
- Stores models in a shared content-addressable layout (ADR 0011)
  under `$MODELS_HOME` (e.g. `%LOCALAPPDATA%\models` on Windows,
  `~/.local/share/models/` on Linux) so multiple tools that adopt
  the convention can reuse the same blobs without re-downloading.

## Who uses it

inferd is plumbing, not a product. Anything on the machine that wants
local inference can sit in front of it: CLI tools, IDE assistants,
agent runtimes, web apps, middleware. Apps don't bundle their own
inference daemon; they connect to inferd.

## Scope

v0.1:

- One backend: local llama.cpp via FFI, Gemma 4 E4B as the reference
  model.
- Frozen wire protocol v1 — `docs/protocol-v1.md`. NDJSON over IPC.
- Rust client crate (`inferd-client`) published to crates.io.
- Hand-written Go client (`clients/go/`); Python and TypeScript
  clients to follow.

v0.2 adds backend adapters for Ollama, OpenAI, Bedrock, Anthropic,
and LiteLLM-compatible servers behind the same `Backend` trait —
turning inferd into a local model-proxy-gateway whose backend is
transparent to every consumer that talks to it.

## Layout

```
inferd/
├── crates/
│   ├── inferd-daemon/      # the binary
│   ├── inferd-proto/       # wire format, published to crates.io
│   ├── inferd-engine/      # backend trait + adapters
│   ├── inferd-client/      # Rust client, published to crates.io
│   └── inferd-stdio/       # stdio variant (no socket, no pipe; later)
├── clients/
│   └── go/                 # hand-written Go client
├── docs/
│   ├── plan-v0.1.md
│   ├── protocol-v1.md
│   └── adr/
└── context.md              # "what is this, why are we building it"
```

## License

MIT. Permissive on purpose — inferd is infrastructure for other tools
to consume.
