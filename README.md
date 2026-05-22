# inferd

> Local inference daemon. One warm model, many consumers.

[![inferd-proto on crates.io](https://img.shields.io/crates/v/inferd-proto?label=inferd-proto)](https://crates.io/crates/inferd-proto)
[![inferd-client on crates.io](https://img.shields.io/crates/v/inferd-client?label=inferd-client)](https://crates.io/crates/inferd-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Status: alpha.** v0.1.0-alpha.0 of `inferd-proto` and `inferd-client`
is on crates.io. The daemon binary ships via GitHub releases. See
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

v0.2 adds backend adapters for OpenAI-compatible servers (vLLM,
LM Studio, LocalAI, llama.cpp's HTTP server, and OpenAI/Anthropic/
Bedrock proper) behind the same `Backend` trait — turning inferd
into a local model-proxy-gateway whose backend is transparent to
every consumer that talks to it.

## Layout

```
inferd/
├── crates/
│   ├── inferd-daemon/      # the binary
│   ├── inferd-proto/       # wire format, published to crates.io
│   ├── inferd-engine/      # backend trait + adapters
│   ├── inferd-client/      # Rust client, published to crates.io
│   └── inferd/             # `inferdctl` CLI binary: status / watch / pull / doctor
├── clients/
│   └── go/                 # hand-written Go client
├── docs/
│   ├── plan-v0.1.md
│   ├── protocol-v1.md
│   └── adr/
└── context.md              # "what is this, why are we building it"
```

## Install

### Linux

inferd ships a per-user systemd unit at
`packaging/systemd/inferd.service`. Install:

```sh
install -Dm755 inferd-daemon ~/.local/bin/inferd-daemon
install -Dm644 packaging/systemd/inferd.service ~/.config/systemd/user/inferd.service
systemctl --user daemon-reload
systemctl --user enable --now inferd
```

The unit declares `RuntimeDirectory=inferd`, so systemd creates
`/run/user/<uid>/inferd/` with the right ownership before
`ExecStart`. Sockets and the lock file live there. The unit also
applies the hardening directives documented in `THREAT_MODEL.md` F-16.

> **Why not `/run/inferd/`?** That directory is for system daemons
> running as root. `systemd --user` cannot write there. inferd
> resolves runtime paths via `$XDG_RUNTIME_DIR` (set by
> `systemd-logind`) on Linux per the algorithm in
> `docs/protocol-v1.md` §"Default endpoint resolution".

#### WSL note

If you previously had a llamafile-style polyglot binary (Cosmopolitan
Libc, `MZ` header) on `PATH` from another tool, remove it before
running inferd inside WSL. WSL's `binfmt_misc` `WSLInterop` handler
matches on the `MZ` magic and tries to run polyglot binaries through
the Windows host, which breaks them. inferd itself ships a normal
ELF (no Cosmopolitan, no `MZ` header), so it is unaffected — but a
stale polyglot binary on `PATH` can still trip the handler if
something execs it.

If you need to disable WSLInterop entirely:

```sh
sudo sh -c 'echo -1 > /proc/sys/fs/binfmt_misc/WSLInterop'   # per-boot
```

Or persistently, add `[interop] enabled = false` to `/etc/wsl.conf`
and run `wsl.exe --shutdown` from Windows.

### macOS

Install the LaunchAgent at `packaging/launchd/io.inferd.daemon.plist`:

```sh
install -m755 inferd-daemon ~/Library/LaunchAgents/inferd-daemon
install -m644 packaging/launchd/io.inferd.daemon.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/io.inferd.daemon.plist
```

### Windows

Run the elevated installer:

```powershell
.\packaging\windows\install.ps1
```

This installs the binary, creates the service via `sc.exe`, and
sets the named-pipe ACL to grant the current user only.

## License

MIT. Permissive on purpose — inferd is infrastructure for other tools
to consume.
