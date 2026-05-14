# inferd

> Local inference daemon. One warm model, many consumers.

**Status: planning.** No code yet. See `docs/plan-v0.1.md` for the
full design and `context.md` for the hand-off brief to the first
implementer.

inferd is a single host-wide Rust service that owns the hard parts of
running a local LLM — loading the model, holding it warm, multiplexing
requests, swapping backends — so that every middleware on the machine
(`thlibo`, future tools) shares one daemon instead of spawning its own.

## Why

Each AI-coding middleware that ships with its own embedded inference
daemon burns RAM and CPU duplicating the same warm model. One
developer laptop running thlibo + a hypothetical second middleware =
two copies of Gemma 4 E4B, ~5 GB each, both busy holding tokens.

inferd solves that by being the *only* local inference endpoint. It:

- Loads a model once, keeps it warm.
- Exposes a small NDJSON-over-IPC protocol on a Unix socket, Windows
  named pipe, or loopback TCP.
- Enforces per-caller identity (UID on Unix, SID on Windows) and an
  optional API key for loopback TCP.
- Multiplexes requests through a single engine or across a pool of
  backend adapters.

## Scope

v0.1 is the minimum to unblock thlibo v0.2:

- One backend: local llamafile, Gemma 4 E4B.
- Wire protocol v1 — identical to thlibo v0.1's NDJSON so migration is
  an import swap, not a protocol rewrite.
- Rust client crate + auto-generated Go/Python/TS clients.

v0.2 adds backend adapters for Ollama, OpenAI, Bedrock, Anthropic, and
LiteLLM-compatible servers — turning inferd into a local
model-proxy-gateway whose backend is transparent to every middleware
that talks to it.

## Layout (planned)

```
inferd/
├── crates/
│   ├── inferd-daemon/      # the binary
│   ├── inferd-proto/       # wire format, published to crates.io
│   ├── inferd-engine/      # backend trait + adapters
│   └── inferd-stdio/       # stdio variant (no socket, no pipe)
├── clients/
│   ├── go/                 # github.com/3rg0n/inferd-go
│   ├── py/
│   └── ts/
├── docs/
│   ├── plan-v0.1.md
│   ├── adr/
│   └── protocol-v1.md
└── context.md              # "what is this, why are we building it"
```

## License

MIT. Permissive on purpose — inferd is infrastructure for other tools
to vendor.

See the related project at [thlibo](https://github.com/3rg0n/thlibo)
for the motivating use case.
