# inferd

> Local inference daemon. One warm model, many consumers.

[![inferd-proto on crates.io](https://img.shields.io/crates/v/inferd-proto?label=inferd-proto)](https://crates.io/crates/inferd-proto)
[![inferd-client on crates.io](https://img.shields.io/crates/v/inferd-client?label=inferd-client)](https://crates.io/crates/inferd-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Status: v0.6.1.** `inferd-proto` and `inferd-client` are on crates.io;
the daemon (`inferd-daemon`), the CLI (`inferdctl`), and the OpenAI-compat
HTTP bridge (`inferd-http`) ship via GitHub releases for Linux (x86_64 +
arm64), macOS arm64, and Windows (x86_64 + arm64) — five platforms. Windows
arm64 was parked at the v0.6.0 tag and ships again from v0.6.1; install=work
is validated on Windows x86_64 CUDA, Linux x86_64 CUDA, and macOS arm64
Metal (see `docs/v0.6-validation.md`). See `context.md` for the hand-off
brief to first-time contributors and `docs/adr/` for the design decisions.

inferd is a single host-wide Rust service that owns the hard parts of
running a local LLM — loading the model, holding it warm, multiplexing
requests, swapping backends, picking the right accelerator — so that
every app on the machine shares one daemon instead of spawning its own.

Since v0.3 the daemon picks the strongest available compute backend
(Metal / CUDA / ROCm / Vulkan / CPU) at runtime from a single binary
(ADR 0019), and ships multimodal by default — the reference Gemma 4
model pulls its multimodal projector on first boot, so a fresh install
answers questions about **images and speech** with no extra config. As of
v0.6 it can also **auto-select the model by accelerator memory** (ADR 0023
— Gemma 4 12B when the accelerator has ≥ 20 GiB, else E4B), and an
**OpenAI-compat HTTP bridge** (`inferd-http`, ADR 0020) ships in every
release so any OpenAI-SDK client — including vision, audio and
structured-output requests — can point at inferd; the bridge is a separate
process, the daemon itself stays IPC-only.

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
  frames for generation (ADR 0021), NDJSON for embeddings (ADR 0017) and
  rerank (ADR 0027).
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

What ships today (v0.6):

- **Local llama.cpp via FFI** (vendored `b9850`), Gemma 4 E4B as the
  reference model — multimodal by default (the projector pulled on first
  boot carries **both** vision and audio) — with **Gemma 4 12B** support
  available.
- **Vision and audio input** on the generation wire: images as raw RGB,
  speech as mono little-endian float32 PCM, each a BLOB-framed attachment
  (ADR 0021). The daemon links no media codec (ADR 0016) — the consumer
  decodes. For audio the consumer also owns **rate conversion**: the
  backend advertises the one sample rate it accepts (`audio_sample_rate`
  on the admin `capabilities` frame; 16 kHz for Gemma 4 E4B) and a
  mismatch is rejected rather than resampled, because libmtmd's audio
  entry point takes no rate argument — so the wrong rate would time-scale
  the clip and return a fluent *wrong* answer instead of an error. Use the
  `inferd-http` bridge if you'd rather not convert audio yourself.
- **Runtime accelerator detection** (ADR 0019): one binary ships every
  ggml backend as a loadable module and picks the strongest at boot.
- **Boot-time model auto-selection** (v0.6, ADR 0023): opt in with
  `model_autoselect: "auto"` and the daemon picks the Gemma 4 variant by
  the accelerator's total memory — ≥ 20 GiB → 12B, else E4B — with a
  pre-load fit check and CPU fallback for embeddings under memory pressure.
- **Three frozen wire surfaces**, each on its own socket: generation
  (v2 — typed content blocks / attachments / tools, ADR 0015) on the
  length-prefixed, type-tagged framing introduced in v0.4 (ADR 0021,
  with raw BLOB media and an in-band `wire_version`), embeddings
  (ADR 0017, NDJSON), and rerank (ADR 0027, NDJSON). The original
  text-only v1 surface was folded into v2 and removed in v0.4.
- **Cross-encoder rerank** (ADR 0027): a fourth socket that scores a
  query against each candidate document *jointly* — one forward pass per
  document, so nothing is precomputable. It sits downstream of retrieval
  (`embed → top-50 → rerank → top-5 → generate`), which is where the
  precision gain over vector similarity alone comes from. Bound only when
  the warm model has a classification head; a cross-encoder GGUF such as
  `bge-reranker-v2-m3` serves it, and Gemma 4 / EmbeddingGemma do not.
- **Structured-output grammar** (ADR 0013): a request may carry a
  `response_format` JSON Schema, which the daemon compiles to a GBNF
  grammar so output is constrained to match the schema. Reachable both on
  the native wire and through the `inferd-http` bridge.
- **No inbound network listener** (ADR 0022): the daemon binds a Unix
  socket / named pipe only; loopback TCP was removed. Network reach is a
  separate bridge process's job (ADR 0020).
- **`inferd-http` OpenAI-compat bridge** (v0.6, ADR 0020): a separate,
  user-launched process — bundled in every release tarball — that exposes
  `/v1/chat/completions` (streaming + non-streaming), `/v1/embeddings`
  (float + base64), `/v1/models`, and `/health`, and translates them to
  the daemon's IPC via `inferd-client`. Supports **vision** (OpenAI
  `image_url` → decoded RGB attachment), **audio** (`input_audio` → decoded
  wav/mp3, downmixed and **resampled** to the rate the daemon requires,
  ADR 0025) and **structured output** (`response_format` json_schema).
  Point OpenCode or any OpenAI-SDK client at it. The daemon serves no HTTP
  itself (ADR 0006); this is a consumer, not a privileged surface
  (ADR 0014) — and the only crate permitted to link MPL-2.0 code
  (`symphonia`, for audio decode), which `deny.toml` enforces in CI.
- **Cloud backend adapters** behind the same `Backend` trait —
  `openai-compat` (vLLM, LM Studio, LocalAI, llama.cpp's HTTP server,
  OpenAI/Anthropic) and `bedrock-invoke` — feature-gated, outbound
  HTTPS only (ADR 0006).
- **Rust client** (`inferd-client`) on crates.io; a hand-written **Go**
  client in `clients/go/` (the canonical non-Rust reference; pin it with
  the path-prefixed module tag, `go get …/clients/go@vX.Y.Z`). Python and
  TypeScript wrappers are planned (stubs in `clients/`).
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
│   ├── inferd-openai-wire/ # OpenAI wire types, shared by both directions
│   ├── inferd-http/        # OpenAI-compat bridge binary (separate process)
│   └── inferd/             # `inferdctl` CLI binary: status / watch / pull / doctor
├── clients/
│   └── go/                 # hand-written Go client (py/ts are README stubs)
├── packaging/              # the per-user installers install=work exercises
├── docs/
│   ├── protocol-v2.md      # normative wire spec (protocol-v1.md is history)
│   ├── test-strategy.md
│   ├── RELEASING.md
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
and auto-pulls the reference model + embedding model + multimodal
projector (vision **and** audio) into the shared model store; watch with
`inferdctl watch`.

Each platform ships **two** archives, the same crates at the same tag
with different build flags:

| Archive | HTTPS client | Models |
|---|---|---|
| `inferd-<ver>-<target>` | linked ([ADR 0010](docs/adr/0010-narrow-https-exception-for-model-bootstrap.md)) | auto-pulled on first boot |
| `inferd-airgapped-<ver>-<target>` | **not linked** | `inferdctl import` only |

The commands below are the same for either one — the airgapped build
just can't fetch, so you import the GGUFs instead of pulling them. See
[docs/airgapped.md](docs/airgapped.md) for that runbook and for the
`cargo tree` assertion that proves the TLS stack is absent rather than
merely unused. `inferd-daemon --version` reports which build is
installed.

### Linux

```sh
tar xzf inferd-v0.6.1-x86_64-unknown-linux-gnu.tar.gz
cd inferd-v0.6.1-x86_64-unknown-linux-gnu
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
tar xzf inferd-v0.6.1-aarch64-apple-darwin.tar.gz
cd inferd-v0.6.1-aarch64-apple-darwin
./packaging/launchd/install-launchagent.sh ./inferd-daemon
inferdctl watch
```

The script flattens the `backends/` modules next to the daemon
(`@loader_path` RPATH resolves them), installs the LaunchAgent, and
bootstraps it. The probe picks Metal on Apple Silicon.

### Windows

```powershell
Expand-Archive inferd-v0.6.1-x86_64-pc-windows-msvc.zip -DestinationPath .
cd inferd-v0.6.1-x86_64-pc-windows-msvc
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
