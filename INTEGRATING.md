# Integrating inferd into your product

This guide is for anyone building a tool that wants local LLM inference and decides to consume `inferd` instead of embedding their own engine. CLI tools, IDE assistants, agent runtimes, web apps, middleware — same pattern.

## What inferd is to your product

**inferd is a local LLM gateway** — Anthropic-API-shaped, but over IPC instead of HTTPS. It owns the model-specific work (chat templating, tokenization decisions, attachment routing, tool-call lifecycle); your product owns the user experience (input capture, rendering, session memory, tool execution).

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
4. Connect to its generation socket from your code, send a length-prefixed v2 request, stream tokens back.

Daemon's contract:
- One generation wire (v2), frozen, on the length-prefixed type-tagged framing introduced in v0.4 ([ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)): typed content blocks, attachments, tools, in-band `wire_version`. The old text-only v1 NDJSON wire was folded into v2 and removed — a text-only request is a single `text` content block. See "[Generation wire](#generation-wire--typed-content-blocks-attachments-tools)" below.
- One warm model per daemon process ([ADR 0012](docs/adr/0012-one-warm-model-per-inferd-process.md)). Need N models? Run N daemons on N socket paths.
- The generation socket only exists when the daemon is `ready`. Connect-refused = not ready = your code's job to wait or passthrough.
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

| Platform | Generation | Admin |
|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/inferd.sock` | `${XDG_RUNTIME_DIR}/inferd/admin.sock` |
| macOS | `${TMPDIR}/inferd/inferd.sock` | `${TMPDIR}/inferd/admin.sock` |
| Windows | `\\.\pipe\inferd` | `\\.\pipe\inferd-admin` |

(The embed surface binds on its own socket when an embed-capable backend is configured — see "[Embeddings](#embeddings)" below.)

The **generation socket** is what your product talks to for `generate` requests. Bound only after the daemon is `ready`.

The **admin socket** is bound earlier (during model load). Subscribe to it for progress events during the first-boot model download — your installer / status UI can show download progress.

### Rust

```toml
[dependencies]
inferd-client = "0.4"
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
```

```rust
use inferd_client::{ClientV2, RequestV2, MessageV2, RoleV2, ContentBlock, ResponseV2, ResponseBlock};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pattern A passive readiness: connect-and-retry until the
    // daemon's generation socket is bound.
    let mut client = inferd_client::dial_and_wait_ready(
        std::time::Duration::from_secs(60),
        || ClientV2::dial_uds(&inferd_client::default_v2_addr()),
    ).await?;

    // A text-only request is a single Text content block. The client
    // stamps `wire_version` for you.
    let mut stream = client.generate(RequestV2 {
        id: "demo-1".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "Hello, who are you?".into() }],
        }],
        max_tokens: Some(64),
        ..Default::default()
    }).await?;

    while let Some(frame) = stream.next().await {
        match frame? {
            ResponseV2::Frame { block: ResponseBlock::Text { delta }, .. } => print!("{delta}"),
            ResponseV2::Frame { .. } => {} // thinking / tool_use blocks
            ResponseV2::Done { backend, stop_reason, .. } => {
                println!("\n[done; backend={backend}, stop={stop_reason:?}]");
            }
            ResponseV2::Error { code, message, .. } => {
                eprintln!("[error {code:?}: {message}]");
            }
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
    return inferd.DialUDS(ctx, inferd.DefaultInferAddr())
})
if err != nil { panic(err) }
defer client.Close()

stream, err := client.GenerateV2(ctx, inferd.RequestV2{
    ID:       "demo-1",
    Messages: []inferd.MessageV2{{Role: inferd.RoleUser, Content: []inferd.ContentBlock{inferd.TextBlock("hello")}}},
})
if err != nil { panic(err) }

for f := range stream {
    switch f.Type {
    case inferd.ResponseV2Frame:
        if f.Block != nil && f.Block.Type == inferd.BlockText {
            fmt.Print(f.Block.Delta)
        }
    case inferd.ResponseV2Done:
        fmt.Printf("\n[done; backend=%s]\n", f.Backend)
    case inferd.ResponseV2Error:
        log.Printf("[error %s: %s]", f.Code, f.Message)
    }
}
```

### Other languages

The generation wire is length-prefixed, type-tagged frames over a Unix socket / Windows named pipe / loopback TCP: `[uvarint payload_len][1 byte type: 0x01 JSON / 0x02 BLOB][payload]`, 64 MiB payload cap. Send a JSON frame (`0x01`) carrying the request (with `"wire_version": 1`); for each attachment send a `BlobDescriptor` JSON frame then a BLOB frame (`0x02`) with the raw bytes. Read response frames the same way. No gRPC, no SSE, no protobuf — a varint codec plus a JSON parser is enough. (The embed surface is simpler still: newline-delimited JSON.)

The Rust types in [`inferd-proto`](https://crates.io/crates/inferd-proto) are the schema reference if you want to generate bindings; ADR 0021 specifies the framing.

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
- **Don't depend on a specific `${XDG_RUNTIME_DIR}` value.** The Rust + Go clients have helpers (`default_v2_addr` / `DefaultInferAddr`) that resolve the chain correctly.
- **PowerShell's default UTF-8 writes a BOM.** If you're poking the daemon from raw PowerShell, use `[System.Text.UTF8Encoding] $false`. The Rust + Go clients don't have this issue.
- **The admin socket has mode `0600`** — only the daemon's own user can connect. The inference socket is `0660` and respects an `inferd-users` group when configured.

## Generation wire — typed content blocks, attachments, tools

The single generation surface (v2) is Anthropic-shaped — typed `messages[].content` blocks carrying text, multimodal, and tool-calling. The content shape is locked in [ADR 0015](docs/adr/0015-v2-wire-protocol-typed-content-blocks.md); the framing is length-prefixed and type-tagged per [ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md). The request JSON frame:

```json
{
  "wire_version": 1,
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
    {"type": "image", "id": "img-1", "width": 768, "height": 512}
  ],
  "tools": [
    {"name": "get_weather", "description": "...", "input_schema": {...}}
  ],
  "max_tokens": 1024
}
```

Recognisable from Anthropic's `/v1/messages`. Borrowed deliberately so middleware authors who've written against Anthropic / OpenAI / Bedrock can write against inferd with the same mental model. Note the attachment metadata carries no `bytes` — the raw bytes ride out-of-band in a BLOB frame (below).

### Framing

Each frame is `[uvarint payload_len][1 byte type][payload]`, type `0x01` = JSON, `0x02` = BLOB, 64 MiB payload cap. A request with no attachments is a single JSON frame. A request with attachments is: the request JSON frame, then for each attachment a `BlobDescriptor` JSON frame (`{"frame":"attachment_blob","attachment_id":"img-1","len":1179648}`) immediately followed by a BLOB frame carrying its raw bytes. The daemon reassembles by `attachment_id`. Responses are length-prefixed JSON frames (`frame` / `done` / `error`).

The `inferd-client` (Rust) and `clients/go` clients do all of this for you — `ClientV2::generate` / `Client.GenerateV2` take a `RequestV2` with attachment bytes attached and emit the frames in order.

### Endpoint

One generation socket (v0.4 / ADR 0021 — the old `infer.sock` / `infer.v2.sock` split is gone):

| Platform | Generation |
|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/inferd.sock` |
| macOS | `${TMPDIR}/inferd/inferd.sock` |
| Windows | `\\.\pipe\inferd` |

It's bound by default as soon as the daemon is `ready` — no flag to flip. (Loopback TCP is opt-in via `--tcp` / `listen.tcp` for cross-VM cases.)

### Attachments are raw bytes, not data URLs

Per [ADR 0016](docs/adr/0016-attachments-are-raw-bytes-the-daemon-doesnt-link-codecs.md), the daemon does **not** link image / audio codecs. Your middleware decodes the user's JPEG / PNG / WAV / MP4 *before* the wire — the attachment carries raw RGB (for images) or PCM (for audio) bytes with the geometry in the JSON metadata. As of v0.4 (ADR 0021) those bytes travel as a raw BLOB frame keyed by `attachment_id`, **not** base64-in-JSON. The daemon hands the bytes to the engine's mtmd helpers verbatim.

This keeps the daemon's binary surface tiny and the threat model narrow (no codec CVEs), and avoids the ~33% base64 inflation on every image. On the encode side it's `image::open(...).resize(...).to_rgb8()` two lines, then hand the `Vec<u8>` to the attachment.

### Tools

Function calling is first-class:

- Define `tools[]` with JSON Schema input descriptors on the request.
- The model's tool-call sequences (`<|tool_call>...<tool_call|>` for Gemma 4; OpenAI's `tool_calls` array for the openai-compat backend) are parsed by the daemon into structured `tool_use` content blocks in the response stream — you never grep raw token streams.
- Send the function results back as `tool_result` blocks in the follow-up request, addressed by `tool_call_id`. The daemon templates the result back into the conversation in the engine-shaped form.

### Migrating from a v1 (pre-v0.4) client

- v1's `Message { role, content: String }` becomes `Message { role, content: Vec<ContentBlock> }` — a text-only turn is a single `ContentBlock::Text`. Same semantic intent, typed array instead of a flat string.
- The transport changed: length-prefixed frames, not NDJSON. Use `inferd-client` 0.4+ (`ClientV2`) / `clients/go` `GenerateV2` — they handle framing + `wire_version` for you. A hand-rolled NDJSON v1 client will not interoperate with a v0.4 daemon.
- Connect to the single generation socket (above). Request id, sampling params, and streaming response handling are otherwise unchanged.

### Backends in v0.2

The router (per [ADR 0007](docs/adr/0007-backend-routing-and-failure-semantics.md)) is now a real priority-ordered policy with per-backend circuit breaker. v0.2 ships three adapters out of the box:

- `llamacpp` — the FFI-linked default; serves any GGUF you put under `$MODELS_HOME` (text + Gemma 4 multimodal + tool-calling, plus embeddings when `embed = true` is set on a llamacpp backend entry).
- `openai-compat` (feature-gated `openai`) — outbound HTTPS to anything that speaks OpenAI Chat Completions: OpenAI itself, vLLM, LM Studio, LocalAI, OpenRouter, llama.cpp's `server`. Same `Backend` trait, same wire on the consumer side. Per ADR 0006, the daemon never *serves* HTTP — this is a narrow outbound carve-out behind the trait.
- `bedrock-invoke` (feature-gated `bedrock`) — outbound HTTPS to AWS Bedrock's `Converse` / `ConverseStream` API for Anthropic / Meta / Mistral / Amazon model families. SigV4-signed; credentials picked up from the standard AWS provider chain.

Apps don't pick the backend — operators do, in `config.json`. There's no per-request `backend` field on the wire ([ADR 0006](docs/adr/0006-lean-core-ecosystem-extensions.md)).

## Embeddings

The embeddings surface is a dedicated socket per [ADR 0017](docs/adr/0017-embeddings-on-a-third-socket.md). Wire shape: single-frame request, single-frame response — no streaming, since an embedding is a complete vector. NDJSON framing (the generation surface's length-prefixed framing doesn't apply here — embeddings never carry BLOBs), 64 MiB cap, same one-warm-model admission slot as generation.

The default embed-capable backend is `llamacpp` configured with `embed: true` and a model that supports it (the [`embeddinggemma-300m`](https://huggingface.co/google/embeddinggemma-300m) GGUF is the reference).

### Endpoint

| Platform | Embed |
|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.embed.sock` |
| macOS | `${TMPDIR}/inferd/infer.embed.sock` |
| Windows | `\\.\pipe\inferd-infer-embed` |

The daemon must be started with `--embed` (or with `INFERD_EMBED=1`) **and** at least one configured backend must advertise `capabilities().embed = true`. Otherwise the embed socket isn't bound — `inferdctl doctor` reports `embed=false` in the capabilities line and the third socket simply isn't present.

### Capability discovery

Subscribe to the admin socket; the daemon emits a `capabilities` frame after backend construction with `embed: true|false` along with `vision`, `audio`, `tools`, `thinking`. `inferdctl doctor` surfaces the same flags so operators can verify before pointing a consumer at the embed path:

```sh
inferdctl doctor
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

- **Generation wire (v2)** is frozen as of v0.4 per [ADR 0021](docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md). New optional fields may appear; older parsers ignore them. A *breaking* change bumps the in-band `wire_version` — the daemon rejects a request whose `wire_version` it doesn't speak with `wire_version_unsupported`, so a mismatch fails loudly instead of corrupting the stream. (This replaced the old "successor on a separate socket" scheme for generation.)
- **Embed wire** is frozen per [ADR 0017](docs/adr/0017-embeddings-on-a-third-socket.md); additive changes only, breaking changes would go to a successor socket.
- **Crate versions** track the daemon: `inferd-proto`, `inferd-engine`, `inferd-client`, and the `inferdctl` CLI all advance together. `inferd-client 0.4.x` always uses `inferd-proto 0.4.x` and works against any `inferd-daemon 0.4.x`.

`cargo add inferd-client` resolves to whatever the latest minor is. **Pre-launch break:** v0.4 changed the generation framing, so a v0.3 client does not interoperate with a v0.4 daemon (and vice versa) — upgrade client and daemon together. After v0.4 the freeze posture returns: the `wire_version` gate keeps later changes from silently breaking older clients.

## Where to file issues

<https://github.com/3rg0n/inferd/issues>. If you're stuck integrating, the integration story is the issue — don't suffer in silence, file it.
