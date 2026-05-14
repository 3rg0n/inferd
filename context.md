# context.md — brief for the implementing Claude

You are being handed a greenfield Rust project called **inferd**. This
doc tells you *why* inferd exists, *what* it has to do, and *where the
reference code lives*. Read this first before touching any file.

## Why inferd exists

A sibling project called **thlibo** ([repo](https://github.com/3rg0n/thlibo))
is a Claude Code + Codex CLI middleware that compresses large tool
output (`git diff`, `npm list`, `cargo test` failures) using a locally-
hosted Gemma 4 E4B model. thlibo v0.1.0 ships with its own inference
daemon baked in (`thlibod`) and its own IPC protocol, both written in
Go.

The problem: if a user installs thlibo *plus* any other AI-coding
middleware on the same machine, each middleware will spawn its own
inference daemon, load its own copy of the model, and fight the other
for RAM and GPU. A typical dev laptop cannot afford two warm 5 GB
models.

inferd fixes this by being the **single local inference endpoint for
the whole machine**. thlibo (and every future middleware) becomes a
thin client. One warm model, many consumers.

## What inferd has to be

A small, hardened Rust daemon that:

1. Loads a configured backend at startup and keeps it warm.
2. Accepts NDJSON-framed requests on a Unix socket, Windows named
   pipe, or loopback TCP — same transport matrix thlibo uses today.
3. Serialises inference through a single active generation + bounded
   queue (exact semantics carried over from thlibo v0.1).
4. Streams tokens back over the same connection.
5. Supports multiple backend adapters behind a single `Backend`
   trait so the operator can switch from local llamafile to Ollama
   to OpenAI to Bedrock without middlewares noticing.
6. Enforces per-caller identity (UID on Unix, SID on Windows) and an
   optional API key for TCP deployments.

For v0.1, **only the local llamafile backend is required**. The
adapter trait must be designed in from day one so v0.2 can add Ollama
+ cloud backends without a rewrite.

## What thlibo gives you (copy-worthy reference code)

You will find a working Go implementation of every piece you need to
port at:

```
github.com/3rg0n/thlibo    (clone it alongside this repo)
  internal/daemon/         # the thing you're replacing — lifecycle,
                           # engine supervisor, queue, lock
  internal/ipc/            # NDJSON framing, socket/pipe/TCP listener,
                           # platform-specific endpoints (Unix, Windows)
  internal/queue/          # fixed-depth admission queue
  internal/logx/           # NDJSON activity log (adopt the same
                           # record shape so ops dashboards work
                           # across middlewares)
  internal/promptsan/      # prompt-injection marker escape (keep it)
  docs/adr/0002-one-warm-model-single-daemon.md   # design invariants
  docs/adr/0003-per-user-autostart-not-system-service.md
  .plan/thlibo-spec.md §"Concurrency", §"IPC", §"Security"
  THREAT_MODEL.md          # every finding that applies to inferd too
```

You are welcome — expected, even — to copy semantics and design
decisions wholesale. The wire format is public-contract; keep it
byte-compatible. The internal Go types are not, but they're a better
starting point than writing from zero.

## Migration contract

The end state is:

- thlibo v0.2 deletes `internal/daemon/` entirely.
- `internal/ipc/` in thlibo gets trimmed to a thin client wrapper or
  replaced with an import of the Go client crate this repo will ship.
- Every existing thlibo integration test that today exercises the
  embedded daemon continues to pass against a running inferd.

The wire protocol you are implementing **must** match thlibo's current
NDJSON frame shape byte-for-byte on day one:

- Request framing: one JSON object per line, `\n`-terminated.
- Fields: `id`, `messages[].{role,content}`, `temperature`, `top_p`,
  `top_k`, `max_tokens`, `stream`, `grammar` (optional; GBNF
  constraint passed through to the engine).
- Response framing: NDJSON frames with a `type` discriminator:
  `token`, `done`, `error`, `status`.
- Image-token-budget validation is carried over: if a message has
  image content, the image budget must be in {70, 140, 280, 560, 1120}
  before the image content.

See `docs/protocol-v1.md` (you will write this early; reference
thlibo's `internal/ipc/protocol.go` and `internal/daemon/lifecycle.go`
for the authoritative shape).

## Invariants you must preserve from thlibo

These are already-paid-for lessons — do not re-open them:

1. **The daemon has zero knowledge of middlewares, processors, hooks,
   or AI clients.** It accepts messages arrays + sampling params,
   streams tokens back. No business logic.
2. **Fallback-on-error is the caller's responsibility.** The daemon
   reports errors cleanly; it does not retry, degrade, or rewrite.
3. **One active generation + bounded queue.** Default 1 active, 10
   queued. `Submit` returns `ErrFull` immediately on overflow.
   Client disconnect cancels the in-flight job.
4. **Single-instance lock** by flock / LockFileEx on a daemon-owned
   lock file. Reject pre-existing symlinks at the lock path (thlibo
   threat-model finding #21).
5. **Sockets are not visible until ready.** Only bind/chmod after the
   engine emits "ready".
6. **No elevation.** Per-user daemon. Unix socket 0660 group
   `thlibo-users` (rename the group to `inferd-users` — thlibo will
   chown to whichever group the operator configures).
7. **NDJSON frame cap.** Per-frame cap at 64 MiB (finding #5) —
   `bufio::read` with an explicit byte limit, not auto-growing buffer.
8. **SHA-256 verification of downloaded models** is constant-time
   (finding #4).
9. **Observability is NDJSON to `~/.inferd/logs/*.ndjson`**, same
   verbosity env var (`INFERD_LOG=0|1|debug`), same rolling rotation
   (keep 3 generations — finding #13), same secret-pattern redactor
   at write time (finding #8).
10. **Every `exec::Command` is reviewed.** The only subprocess inferd
    spawns is the llamafile backend, and the path comes from operator
    config, not a client. All other "exec" is a code smell.

## Where to start

Read, in order:

1. This file (done).
2. `docs/plan-v0.1.md` — the crate structure, milestone breakdown, and
   exact responsibilities of each crate.
3. `github.com/3rg0n/thlibo/.plan/thlibo-spec.md` — the daemon half
   of that spec is what you are porting.
4. `github.com/3rg0n/thlibo/THREAT_MODEL.md` — every finding marked
   "L2", "L4", "L5", "L6" in thlibo applies to inferd. The
   remediations are in thlibo's code; port the remediation, not just
   the feature.
5. `github.com/3rg0n/thlibo/internal/daemon/lifecycle.go` — the main
   loop you are re-implementing. It's ~550 lines and it handles
   boot, accept, admission, dispatch, streaming, restart, and clean
   shutdown. Your Rust version should be recognisably the same
   shape.

Then propose a v0.0 milestone: a daemon that loads nothing, accepts a
connection, echoes back the request as a `done` frame. That proves
the transport. From there, bring up the llamafile backend, then the
`Backend` trait, then the remaining adapters.

## What NOT to do

- Don't invent a new wire protocol. Match thlibo's v1 exactly.
- Don't add features thlibo doesn't need yet. v0.1 of inferd is
  "drop-in replacement for thlibod", not "the future of local AI".
- Don't introduce async runtime pluralism. Pick one (tokio is the
  obvious call) and use it everywhere.
- Don't embed llamafile via FFI in v0.1. Keep spawning it as a
  subprocess, same as thlibo does — the Mozilla binary is already
  vendored into the thlibo release and the contract is stable.
- Don't make the daemon speak HTTP. If a middleware needs HTTP it
  can put an HTTP→IPC adapter in its own process.

## Who asked for this

The thlibo maintainer (you'll find `3rg0n` in the git log of both
repos) and the user this conversation originated from. When you have
questions about intent that aren't answered by `docs/plan-v0.1.md` or
the thlibo spec, open an ADR draft with the question and your
proposed answer — don't guess silently.
