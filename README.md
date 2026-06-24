# inferd

> Local inference daemon. One warm model, many consumers.

[![inferd-proto on crates.io](https://img.shields.io/crates/v/inferd-proto?label=inferd-proto)](https://crates.io/crates/inferd-proto)
[![inferd-client on crates.io](https://img.shields.io/crates/v/inferd-client?label=inferd-client)](https://crates.io/crates/inferd-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Status: v0.5.0.** `inferd-proto` and `inferd-client` are on crates.io;
the daemon binary ships via GitHub releases for Linux (x86_64 + arm64),
macOS arm64, and Windows (x86_64 + arm64). See `context.md` for the
hand-off brief to first-time contributors and `docs/adr/` for the design
decisions.

inferd is a single host-wide Rust service that owns the hard parts of
running a local LLM — loading the model, holding it warm, multiplexing
requests, swapping backends, picking the right accelerator — so that
every app on the machine shares one daemon instead of spawning its own.

Since v0.3 the daemon picks the strongest available compute backend
(Metal / CUDA / ROCm / Vulkan / CPU) at runtime from a single binary
(ADR 0019), and ships multimodal by default — the reference Gemma 4
model pulls its vision projector on first boot, so a fresh install
answers questions about images with no extra config.

## Why

Every app that embeds its own inference engine burns RAM and CPU
duplicating the same warm model. One developer laptop running two
such apps = two copies of a multi-GB model, both busy holding tokens.
The same shape applies on a server: every web app that talks to a
local LLM ends up reinventing the same warm-model lifecycle.

inferd solves that by being the *only* local inference endpoint on the
host. It:

- Loads a model once, keeps it warm.
- Exposes a small IPC protocol on a Unix socket or Windows named pipe
  (no inbound network listener, ADR 0022) — length-prefixed, type-tagged
  frames for generation (ADR 0021), NDJSON for embeddings (ADR 0017).
- Enforces per-caller identity via kernel-attested peer credentials
  (UID on Unix, SID on Windows).
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

What ships today (v0.5):

- **Local llama.cpp via FFI**, Gemma 4 E4B as the reference model —
  multimodal by default (vision projector pulled on first boot).
- **Runtime accelerator detection** (ADR 0019): one binary ships every
  ggml backend as a loadable module and picks the strongest at boot.
- **Two frozen wire surfaces**, each on its own socket: generation
  (v2 — typed content blocks / attachments / tools, ADR 0015) on the
  length-prefixed, type-tagged framing introduced in v0.4 (ADR 0021,
  with raw BLOB media and an in-band `wire_version`), and embeddings
  (ADR 0017, NDJSON). The original text-only v1 surface was folded into
  v2 and removed in v0.4.
- **Structured-output grammar** (v0.5, ADR 0013): a request may carry a
  `response_format` JSON Schema, which the daemon compiles to a GBNF
  grammar so output is constrained to match the schema.
- **No inbound network listener** (v0.5, ADR 0022): the daemon binds a
  Unix socket / named pipe only; loopback TCP was removed. Network reach
  is a separate bridge process's job (ADR 0020).
- **Cloud backend adapters** behind the same `Backend` trait —
  `openai-compat` (vLLM, LM Studio, LocalAI, llama.cpp's HTTP server,
  OpenAI/Anthropic) and `bedrock-invoke` — feature-gated, outbound
  HTTPS only (ADR 0006).
- **Rust client** (`inferd-client`) on crates.io; hand-written Go,
  Python, and TypeScript clients in `clients/`.
- **`inferdctl`** CLI: `status` / `watch` / `pull` / `doctor`.

Everything is one host-wide daemon: apps connect to inferd instead of
bundling their own engine. HTTP / OpenAI-compat *server* surfaces stay
out of the daemon by design (ADR 0006) and live as separate
ecosystem-extension processes (ADR 0020).

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

Download the tarball for your platform from the
[releases page](https://github.com/3rg0n/inferd/releases) and run the
bundled per-user installer. **No elevation** on any platform — inferd
runs as a per-user service (systemd `--user` / launchd LaunchAgent /
Windows Startup-folder), stops at logout, and never touches a
system-wide service. On first boot it writes `~/.inferd/config.json`
and auto-pulls the reference model + embedding model + vision projector
into the shared model store; watch with `inferdctl watch`.

### Linux

```sh
tar xzf inferd-v0.5.0-x86_64-unknown-linux-gnu.tar.gz
cd inferd-v0.5.0-x86_64-unknown-linux-gnu
mkdir -p ~/.local/bin ~/.config/systemd/user
cp -f inferd-daemon inferdctl ~/.local/bin/
cp -f backends/* ~/.local/bin/            # ggml backend modules ($ORIGIN RPATH)
cp -f packaging/inferd.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now inferd
inferdctl watch                            # first-boot model pull
```

The unit declares `RuntimeDirectory=inferd`, so systemd creates
`/run/user/<uid>/inferd/` with the right ownership before `ExecStart`;
sockets and the lock file live there. It applies the hardening
directives documented in `THREAT_MODEL.md` F-16.

> **Why not `/run/inferd/`?** That directory is for system daemons
> running as root. `systemd --user` cannot write there. inferd
> resolves runtime paths via `$XDG_RUNTIME_DIR` (set by
> `systemd-logind`) on Linux, falling back to `$HOME/.inferd/run/`
> then `/tmp/inferd/` (see `endpoint::default_addr` / ADR 0021).

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

### macOS (Apple Silicon)

```sh
tar xzf inferd-v0.5.0-aarch64-apple-darwin.tar.gz
cd inferd-v0.5.0-aarch64-apple-darwin
./packaging/launchd/install-launchagent.sh ./inferd-daemon
inferdctl watch
```

The script flattens the `backends/` modules next to the daemon
(`@loader_path` RPATH resolves them), installs the LaunchAgent, and
bootstraps it. The probe picks Metal on Apple Silicon.

### Windows

```powershell
Expand-Archive inferd-v0.5.0-x86_64-pc-windows-msvc.zip -DestinationPath .
cd inferd-v0.5.0-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\packaging\install.ps1 `
    -SourceBinary .\inferd-daemon.exe
inferdctl watch
```

Per-user, **no elevation**: the installer stages the binary +
`backends\` DLLs into `%LOCALAPPDATA%\inferd`, registers a Startup-
folder shortcut, and launches the daemon (named pipes, default DACL
granting the current user). The CUDA build resolves its redist DLLs
next to the exe; no system-wide CUDA install needed.

## License

MIT. Permissive on purpose — inferd is infrastructure for other tools
to consume.
