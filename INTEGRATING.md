# Integrating inferd into your product

This guide is for anyone building a tool that wants local LLM inference and decides to consume `inferd` instead of embedding their own engine. CLI tools, IDE assistants, agent runtimes, web apps, middleware — same pattern.

## What inferd is to your product

**inferd is a local LLM gateway** — Anthropic-API-shaped, but over IPC instead of HTTPS. It owns the model-specific work (chat templating, tokenization decisions, attachment routing once v2 ships, tool-call lifecycle); your product owns the user experience (input capture, rendering, session memory, tool execution).

Mental model:

```
[ your middleware ]              [ inferd daemon ]            [ llama.cpp + model ]
  thin client                      smart gateway                math
  - knows the user                 - knows the model            - tokens in
  - knows the task                 - shapes intent → engine     - tokens out
  - sends semantic intent          - manages lifecycle
  - renders streamed tokens        - routes attachments
  - executes tools                 - orchestrates tool calls
```

This is the same split Anthropic's `/v1/messages` API draws between Claude Code and Anthropic's models. It's exactly the split [ADR 0013](docs/adr/0013-inferd-is-the-gateway-not-the-pipe.md) commits inferd to. Your middleware doesn't write `<|turn>...<turn|>` chat-template tokens by hand and doesn't compute image embeddings — the daemon does. You send semantic `messages[]`, the daemon produces engine-shaped input.

## TL;DR

1. Install the daemon binary on the user's machine. Pre-built tarballs at [GitHub Releases](https://github.com/3rg0n/inferd/releases).
2. Write `~/.inferd/config.json` with the model you want.
3. Start the daemon as a service (systemd / launchd / Windows service — units shipped in the tarball).
4. Connect to its inference socket from your code, send NDJSON, stream tokens back.

Daemon's contract:
- Wire protocol v1 is frozen and text-only (see [`docs/protocol-v1.md`](docs/protocol-v1.md)). v2 (typed content blocks + attachments + tools) lives on a separate socket per [ADR 0008](docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md) — shipping in v0.2.0; see "[v2 wire (v0.2)](#v2-wire-v02--typed-content-blocks-attachments-tools)" below.
- One warm model per daemon process ([ADR 0012](docs/adr/0012-one-warm-model-per-inferd-process.md)). Need N models? Run N daemons on N socket paths.
- The inference socket only exists when the daemon is `ready`. Connect-refused = not ready = your code's job to wait or passthrough.
- Errors: callers own retry. Daemon never retries, never fails over, never rewrites.

## Step 1 — install the daemon

### Linux

```sh
# Download from releases
TAG=v0.2.0
ARCH=$(uname -m)  # x86_64 or aarch64
curl -L -o /tmp/inferd.tar.gz \
  "https://github.com/3rg0n/inferd/releases/download/${TAG}/inferd-${TAG}-${ARCH}-unknown-linux-gnu.tar.gz"
tar xzf /tmp/inferd.tar.gz -C /tmp
mkdir -p ~/.local/bin ~/.config/systemd/user
install -m755 /tmp/inferd-${TAG}-${ARCH}-unknown-linux-gnu/inferd-daemon ~/.local/bin/inferd-daemon
install -m644 /tmp/inferd-${TAG}-${ARCH}-unknown-linux-gnu/packaging/inferd.service \
  ~/.config/systemd/user/inferd.service
systemctl --user daemon-reload
systemctl --user enable --now inferd
```

### macOS

```sh
TAG=v0.2.0
curl -L -o /tmp/inferd.tar.gz \
  "https://github.com/3rg0n/inferd/releases/download/${TAG}/inferd-${TAG}-aarch64-apple-darwin.tar.gz"
tar xzf /tmp/inferd.tar.gz -C /tmp
mkdir -p ~/Library/LaunchAgents ~/.local/bin
install -m755 /tmp/inferd-${TAG}-aarch64-apple-darwin/inferd-daemon ~/.local/bin/inferd-daemon
install -m644 /tmp/inferd-${TAG}-aarch64-apple-darwin/packaging/io.inferd.daemon.plist \
  ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/io.inferd.daemon.plist
```

### Windows (PowerShell, elevated)

```powershell
$tag = "v0.2.0"
$url = "https://github.com/3rg0n/inferd/releases/download/$tag/inferd-$tag-x86_64-pc-windows-msvc.zip"
$tmp = "$env:TEMP\inferd-$tag.zip"
Invoke-WebRequest -Uri $url -OutFile $tmp
Expand-Archive -Path $tmp -DestinationPath $env:TEMP
& "$env:TEMP\inferd-$tag-x86_64-pc-windows-msvc\packaging\install.ps1"
```

## Step 2 — write `~/.inferd/config.json`

Default location: `$HOME/.inferd/config.json` on Unix, `%USERPROFILE%\.inferd\config.json` on Windows. Override with `--config` or `INFERD_CONFIG`.

```json
{
  "auto_pull": true,
  "model": {
    "name": "gemma-4-e4b",
    "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
    "size_bytes": 5126304928,
    "source_url": "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/gemma-4-E4B-it-UD-Q4_K_XL.gguf",
    "license": "apache-2.0"
  },
  "n_ctx": 8192,
  "n_gpu_layers": 0
}
```

`auto_pull: true` (the default) means: on first start, download the GGUF from `source_url`, verify SHA-256 with constant-time compare, store in the shared CAS layout under `$MODELS_HOME` (per [ADR 0011](docs/adr/0011-shared-content-addressable-model-store.md)), then load and serve.

The model named here is the only model that daemon serves. Want a second model? Run a second daemon with a different config + different socket path.

## Step 3 — connect

### Default endpoint paths

| Platform | Inference | Admin |
|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.sock` | `${XDG_RUNTIME_DIR}/inferd/admin.sock` |
| macOS | `${TMPDIR}/inferd/infer.sock` | `${TMPDIR}/inferd/admin.sock` |
| Windows | `\\.\pipe\inferd-infer` | `\\.\pipe\inferd-admin` |

(v2 and embed bind on additional sockets when enabled — see "[v2 wire (v0.2)](#v2-wire-v02--typed-content-blocks-attachments-tools)" and "[Embeddings (v0.2)](#embeddings-v02)" below.)

The **inference socket** is what your product talks to for `generate` requests. Bound only after the daemon is `ready`.

The **admin socket** is bound earlier (during model load). Subscribe to it for progress events during the first-boot model download — your installer / status UI can show download progress.

### Rust

```toml
[dependencies]
# v0.1: text-only v1 wire. v0.2: same v1 surface plus v2 typed content
# blocks. Pick the line that matches the wire you're targeting.
inferd-client = "0.2"
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
```

```rust
use inferd_client::{Client, Request, Message, Role, Response};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pattern A passive readiness: connect-and-retry until the
    // daemon's inference socket is bound.
    let mut client = inferd_client::dial_and_wait_ready(
        std::time::Duration::from_secs(60),
        || Client::dial_uds(std::path::Path::new("/run/user/1000/inferd/infer.sock")),
    ).await?;

    let mut stream = client.generate(Request {
        id: "demo-1".into(),
        messages: vec![Message {
            role: Role::User,
            content: "Hello, who are you?".into(),
        }],
        max_tokens: Some(64),
        ..Default::default()
    }).await?;

    while let Some(frame) = stream.next().await {
        match frame? {
            Response::Token { content, .. } => print!("{content}"),
            Response::Done { backend, stop_reason, .. } => {
                println!("\n[done; backend={backend}, stop={stop_reason:?}]");
            }
            Response::Error { code, message, .. } => {
                eprintln!("[error {code:?}: {message}]");
            }
            Response::Status { .. } => {}
        }
    }
    Ok(())
}
```

### Go

```go
import "github.com/3rg0n/inferd/clients/go"

ctx, _ := context.WithTimeout(context.Background(), 60*time.Second)
client, err := inferd.DialAndWaitReady(ctx, func() (*inferd.Client, error) {
    return inferd.DialUDS("/run/user/1000/inferd/infer.sock")
})
if err != nil { panic(err) }
defer client.Close()

stream, err := client.Generate(ctx, &inferd.Request{
    ID:        "demo-1",
    Messages:  []inferd.Message{{Role: "user", Content: "hello"}},
    MaxTokens: 64,
})
if err != nil { panic(err) }

for stream.Next() {
    switch f := stream.Frame().(type) {
    case *inferd.TokenFrame:
        fmt.Print(f.Content)
    case *inferd.DoneFrame:
        fmt.Printf("\n[done; backend=%s]\n", f.Backend)
    case *inferd.ErrorFrame:
        log.Printf("[error %s: %s]", f.Code, f.Message)
    }
}
```

### Other languages

The wire format is plain NDJSON over a Unix socket / Windows named pipe / loopback TCP. No gRPC, no SSE, no protobuf. `socket.connect()` + `send(json + "\n")` + `recv()` works in any language with a JSON parser.

The Rust types in [`inferd-proto`](https://crates.io/crates/inferd-proto) are the schema reference if you want to generate bindings.

## Wait-for-ready patterns

### Pattern A — passive (recommended for inference-only consumers)

Retry connect against the inference socket. Successful connect IS the readiness signal because the daemon's [F-13](THREAT_MODEL.md) ordering guarantees the socket only exists when the backend is `ready`.

`inferd-client::dial_and_wait_ready` and Go's `inferd.DialAndWaitReady` implement this.

### Pattern B — active (for installer GUIs and progress UX)

Subscribe to the **admin socket**. The daemon publishes lifecycle events (`starting`, `loading_model { phase: download | verify | mmap | kv_cache }`, `ready`, `restarting`, `draining`) as NDJSON frames.

Use this when you want to display "downloading 4.2 GB / 5.0 GB ..." progress during the first-boot model fetch.

```rust
use inferd_client::{AdminClient, AdminEvent};

let mut admin = AdminClient::dial_admin_uds(
    std::path::Path::new("/run/user/1000/inferd/admin.sock")
).await?;
loop {
    let event: AdminEvent = admin.recv().await?;
    match event.status.as_str() {
        "loading_model" if event.phase == "download" => {
            let pct = event.downloaded_bytes
                .zip(event.total_bytes)
                .map(|(d, t)| 100.0 * d as f64 / t as f64);
            println!("downloading: {pct:?}");
        }
        "ready" => break,
        _ => {}
    }
}
```

## Error contract

Three terminal outcomes for a request:

1. **`done` frame** — generation completed normally. `stop_reason` tells you why (length cap, EOS token, etc.).
2. **`error` frame** — daemon refused or aborted. Structured `code`:
   - `queue_full` — admission queue saturated. Retry with backoff.
   - `backend_unavailable` — backend not ready or crashed mid-stream. Retry, possibly switch to passthrough.
   - `invalid_request` — your request didn't validate. Don't retry; fix the client.
   - `internal` — daemon bug. File an issue.
3. **EOF** — connection closed without a terminal frame. Equivalent to `backend_unavailable`; retry policy is yours.

The daemon never retries on its own. It never falls over to a different backend mid-stream. If you want fallback / retry / passthrough, you build it.

## Gotchas

- **Daemon must be running before your code connects.** If your installer starts the daemon and your app immediately connects, use Pattern A's retry — there's a race during bring-up.
- **Don't depend on a specific `${XDG_RUNTIME_DIR}` value.** The Rust + Go clients have helpers (`default_inference_addr` / `DefaultInferenceAddr`) that resolve the chain correctly.
- **PowerShell's default UTF-8 writes a BOM.** If you're poking the daemon from raw PowerShell, use `[System.Text.UTF8Encoding] $false`. The Rust + Go clients don't have this issue.
- **The admin socket has mode `0600`** — only the daemon's own user can connect. The inference socket is `0660` and respects an `inferd-users` group when configured.

## v2 wire (v0.2) — typed content blocks, attachments, tools

v0.2 ships an Anthropic-shaped wire protocol on a *separate* socket alongside v1. v1 stays frozen forever (text-only `messages[].content` as a `String`); v2 carries multimodal + tool-calling without breaking anything you build today.

The shape is locked in [ADR 0015](docs/adr/0015-v2-wire-protocol-typed-content-blocks.md). On the wire:

```json
{
  "id": "req-001",
  "messages": [
    {
      "role": "user",
      "content": [
        {"type": "text", "text": "What's in this image?"},
        {"type": "image", "attachment_id": "img-1"}
      ]
    }
  ],
  "attachments": [
    {"id": "img-1", "kind": "image", "mime": "image/jpeg", "bytes": "<base64>"}
  ],
  "tools": [
    {"name": "get_weather", "description": "...", "input_schema": {...}}
  ],
  "max_tokens": 1024
}
```

Recognisable from Anthropic's `/v1/messages`. Borrowed deliberately so middleware authors who've written against Anthropic / OpenAI / Bedrock can write against inferd with the same mental model.

### Endpoint

v2 binds on its own socket alongside v1:

| Platform | v1 inference | v2 inference |
|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.sock` | `${XDG_RUNTIME_DIR}/inferd/infer.v2.sock` |
| macOS | `${TMPDIR}/inferd/infer.sock` | `${TMPDIR}/inferd/infer.v2.sock` |
| Windows | `\\.\pipe\inferd-infer` | `\\.\pipe\inferd-infer-v2` |

The daemon must be started with `--v2` (or `INFERD_V2=1`) for the v2 socket to be bound. The shipped systemd / launchd / Windows units are v1-only by default; flip the flag in your operator config when you're ready.

### Attachments are raw bytes, not data URLs

Per [ADR 0016](docs/adr/0016-attachments-are-raw-bytes-the-daemon-doesnt-link-codecs.md), the daemon does **not** link image / audio codecs. Your middleware decodes the user's JPEG / PNG / WAV / MP4 *before* the wire — `attachment.bytes` is base64'd raw RGB (for images) or PCM (for audio), with the geometry passed in the surrounding wire fields. The daemon then hands those bytes to the engine's mtmd helpers verbatim.

This keeps the daemon's binary surface tiny and the threat model narrow (no codec CVEs). The Rust client in 0.2 grows helpers to do the encoding-side work for the common formats; until then, it's `image::open(...).resize(...).to_rgb8()` two lines.

### Tools

Function calling is first-class:

- Define `tools[]` with JSON Schema input descriptors on the request.
- The model's tool-call sequences (`<|tool_call>...<tool_call|>` for Gemma 4; OpenAI's `tool_calls` array for the openai-compat backend) are parsed by the daemon into structured `tool_use` content blocks in the response stream — you never grep raw token streams.
- Send the function results back as `tool_result` blocks in the follow-up request, addressed by `tool_call_id`. The daemon templates the result back into the conversation in the engine-shaped form.

### What this means for v0.1 middleware authors

- Today's `Message { role, content: String }` → v2's `Message { role, content: Vec<ContentBlock> }`. The same semantic intent as a typed array instead of a flat string.
- v1 stays valid. If you don't need multimodal or tools, you don't have to migrate. The v1 socket keeps serving the same wire forever.
- When you migrate, the migration is local: `Message` keeps its `role` field, gains a `content: Vec<ContentBlock>` instead of `content: String`. Request id, sampling params, streaming response handling — unchanged.
- Connect to the v2 endpoint path (above), not the v1 path. The Rust client's v2 surface lands as `inferd-client = "0.2"` with the same dial-and-wait semantics.

### Backends in v0.2

The router (per [ADR 0007](docs/adr/0007-backend-routing-and-failure-semantics.md)) is now a real priority-ordered policy with per-backend circuit breaker. v0.2 ships three adapters out of the box:

- `llamacpp` — the FFI-linked default; serves any GGUF you put under `$MODELS_HOME` (text + Gemma 4 multimodal + tool-calling, plus embeddings when `embed = true` is set on a llamacpp backend entry).
- `openai-compat` (feature-gated `openai`) — outbound HTTPS to anything that speaks OpenAI Chat Completions: OpenAI itself, vLLM, LM Studio, LocalAI, OpenRouter, llama.cpp's `server`. Same `Backend` trait, same wire on the consumer side. Per ADR 0006, the daemon never *serves* HTTP — this is a narrow outbound carve-out behind the trait.
- `bedrock-invoke` (feature-gated `bedrock`) — outbound HTTPS to AWS Bedrock's `Converse` / `ConverseStream` API for Anthropic / Meta / Mistral / Amazon model families. SigV4-signed; credentials picked up from the standard AWS provider chain.

Apps don't pick the backend — operators do, in `config.json`. There's no per-request `backend` field on the wire ([ADR 0006](docs/adr/0006-lean-core-ecosystem-extensions.md)).

## Embeddings (v0.2)

v0.2 adds an embeddings surface on a *third* dedicated socket per [ADR 0017](docs/adr/0017-embeddings-on-a-third-socket.md). Wire shape: single-frame request, single-frame response — no streaming, since an embedding is a complete vector. Same NDJSON, same 64 MiB cap, same one-warm-model admission slot as v1 / v2.

The default embed-capable backend is `llamacpp` configured with `embed: true` and a model that supports it (the [`embeddinggemma-300m`](https://huggingface.co/google/embeddinggemma-300m) GGUF is the v0.2 reference).

### Endpoint

| Platform | Embed |
|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.embed.sock` |
| macOS | `${TMPDIR}/inferd/infer.embed.sock` |
| Windows | `\\.\pipe\inferd-infer-embed` |

The daemon must be started with `--embed` (or with `INFERD_EMBED=1`) **and** at least one configured backend must advertise `capabilities().embed = true`. Otherwise the embed socket isn't bound — `inferd doctor` reports `embed=false` in the capabilities line and the third socket simply isn't present.

### Capability discovery

Subscribe to the admin socket; the daemon emits a `capabilities` frame after backend construction with `embed: true|false` along with `vision`, `audio`, `tools`, `thinking`. `inferd doctor` surfaces the same flags so operators can verify before pointing a consumer at the embed path:

```sh
inferd doctor
# [ ok ] backend: llamacpp accelerator=cuda gpu_layers=99 v2=true vision=true audio=false tools=true thinking=true embed=true
```

### Config-file shape

Add `embed: true` to the `llamacpp` backend entry that should serve embeddings (the same backend can also serve generation, but using a dedicated embedding model is recommended — embeddinggemma-300m is too small to generate well):

```json
{
  "backends": [
    {
      "kind": "llamacpp",
      "name": "embeddings",
      "embed": true,
      "embed_pooling": 1,
      "embed_n_ctx": 2048,
      "model": {
        "name": "embeddinggemma-300m",
        "sha256": "<...>",
        "size_bytes": 305000000,
        "source_url": "https://huggingface.co/.../embeddinggemma-300m.gguf",
        "license": "gemma"
      }
    }
  ]
}
```

`embed_pooling` defaults to `1` (`LLAMA_POOLING_TYPE_MEAN`) — what EmbeddingGemma expects. `embed_n_ctx` defaults to `2048`. The adapter allocates a *second* `llama_context` configured with `embeddings = true` so embedding requests don't race the generation context — generation and embedding can run concurrently against the same model.

### Rust

```rust
use inferd_client::{EmbedClient, EmbedRequest, EmbedResponse, EmbedTask};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = inferd_client::dial_and_wait_ready(
        std::time::Duration::from_secs(30),
        || EmbedClient::dial_uds(&inferd_client::default_embed_addr()),
    ).await?;

    let resp = client.embed(EmbedRequest {
        id: "demo-1".into(),
        input: vec!["the quick brown fox".into()],
        dimensions: Some(256),
        task: Some(EmbedTask::RetrievalDocument),
    }).await?;

    match resp {
        EmbedResponse::Embeddings { embeddings, dimensions, model, .. } => {
            println!("{model}: {} vectors of dim {dimensions}", embeddings.len());
        }
        EmbedResponse::Error { code, message, .. } => {
            eprintln!("[embed error {code:?}: {message}]");
        }
    }
    Ok(())
}
```

### What you can ask for

- **`input`** — one or more strings, encoded independently. `embeddings[i]` corresponds to `input[i]`.
- **`dimensions`** — Matryoshka truncation length. EmbeddingGemma supports `768 | 512 | 256 | 128`. Backends validate against their own supported set; rejected values return `invalid_request`. Omitted means "model default".
- **`task`** — task-prefix hint applied at the engine layer per ADR 0013. EmbeddingGemma uses task-aware prefixes (`retrieval_query`, `retrieval_document`, `similarity`, `classification`, `clustering`, `question_answering`, `fact_verification`, `code_retrieval_query`); the daemon prepends the engine-specific text on your behalf. Backends that don't apply task prefixes ignore the hint.

### Error contract (embed)

Single terminal frame, two outcomes — `embeddings` (success) or `error` with a machine-readable `code` from `EmbedErrorCode`:

- `queue_full` — admission queue saturated; retry with backoff.
- `backend_unavailable` — the embed-capable backend isn't ready or errored.
- `invalid_request` — empty input, unsupported `dimensions`, unknown `task` (e.g. an `Other` variant from a future client). Don't retry; fix the request.
- `frame_too_large` — request exceeded the 64 MiB cap.
- `embed_unsupported` — fail-safe; the active backend doesn't support embeddings. (You shouldn't see this in practice — the embed socket isn't bound when no backend can serve.)
- `internal` — daemon bug.

## Versioning

inferd follows semver:

- **v1 wire** is frozen and immutable. New optional fields may appear; older parsers ignore them. Breaking changes go to v2 on a separate socket path (per [ADR 0008](docs/adr/0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md)).
- **v2 wire** lands in `0.2.x` per [ADR 0015](docs/adr/0015-v2-wire-protocol-typed-content-blocks.md). v2 is also frozen once shipped — additive changes only; further breaking shapes become v3 on yet another socket.
- **embed wire** lands in `0.2.x` per [ADR 0017](docs/adr/0017-embeddings-on-a-third-socket.md). Same separate-socket-per-surface pattern; frozen once shipped.
- **Crate versions** track the daemon: `inferd-proto`, `inferd-engine`, `inferd-client`, and the `inferd` CLI all advance together. `inferd-client 0.2.x` always uses `inferd-proto 0.2.x` and works against any `inferd-daemon 0.2.x`.

`cargo add inferd-client` resolves to whatever the latest minor is. v1 consumers using `inferd-client = "0.1"` keep working unchanged against the v1 socket of a v0.2 daemon — the daemon binds both sockets at the same time when `--v2` is set, and v1's wire is unchanged.

## Where to file issues

<https://github.com/3rg0n/inferd/issues>. If you're stuck integrating, the integration story is the issue — don't suffer in silence, file it.
