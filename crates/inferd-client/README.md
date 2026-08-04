# inferd-client

Rust client for the [inferd](https://github.com/3rg0n/inferd)
local-inference daemon.

Three frozen wire surfaces, each on its own socket: generation
(`ClientV2` — typed content blocks / attachments / tools, ADR 0015)
on the length-prefixed, type-tagged framing introduced in v0.4
([ADR 0021](https://github.com/3rg0n/inferd/blob/main/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)),
embeddings (`EmbedClient`, ADR 0017, NDJSON), and cross-encoder rerank
(`RerankClient`, [ADR 0027](https://github.com/3rg0n/inferd/blob/main/docs/adr/0027-reranking-on-a-fourth-socket.md),
NDJSON). The original text-only v1 surface was folded into `ClientV2`
and removed in v0.4.

A socket exists only when the warm model advertises that capability, so
a failed connect is capability discovery, not an outage — and one daemon
serves one model (ADR 0012), so generation + embeddings + rerank means
three daemon processes.

## Install the daemon first

The client connects to a **running `inferd-daemon`**. You install the
daemon out-of-band; this crate doesn't bundle it.

Pre-built binaries (Linux x86_64 + arm64, macOS arm64, Windows
x86_64) ship with each release at
<https://github.com/3rg0n/inferd/releases>. Each tarball signed with
cosign keyless OIDC.

The daemon defaults to `auto_pull: true`, which means on first start
it downloads the configured model from the configured `source_url`,
verifies SHA-256 with constant-time compare, then mmaps and starts
serving. Watch progress on the admin socket (Pattern B below) or
the daemon's stdout if you're running it directly.

## Quickstart

```rust,no_run
use inferd_client::{ClientV2, RequestV2, MessageV2, RoleV2, ContentBlock, ResponseV2, ResponseBlock};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pattern A: connect-and-retry against the generation socket.
    // The successful connect IS the readiness signal — F-13 in the
    // upstream threat model guarantees the generation socket only
    // exists when the daemon is `ready`.
    let mut client = inferd_client::dial_and_wait_ready(
        std::time::Duration::from_secs(30),
        || ClientV2::dial_uds(&inferd_client::default_v2_addr()), // Windows: ClientV2::dial_pipe(r"\\.\pipe\inferd")
    )
    .await?;

    // Text-only is a single Text content block; the client stamps
    // `wire_version` for you.
    let mut stream = client
        .generate(RequestV2 {
            id: "demo-1".into(),
            messages: vec![MessageV2 {
                role: RoleV2::User,
                content: vec![ContentBlock::Text { text: "hello".into() }],
            }],
            ..Default::default()
        })
        .await?;

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

## Attachments (vision + audio)

Put the decoded bytes in `RequestV2::attachments`, then reference each
one by `id` from a content block. `generate()` splits them for you: the
request JSON frame carries only metadata, and each attachment's bytes
follow as a `BlobDescriptor` + BLOB frame pair (ADR 0021). The daemon
links no media codec ([ADR 0016](https://github.com/3rg0n/inferd/blob/main/docs/adr/0016-consumer-decodes-media-before-sending.md)),
so "decoded" is literal — raw RGB, never a PNG; float32 PCM, never a WAV.

```rust,ignore
use inferd_client::{Attachment, ContentBlock, MessageV2, RequestV2, RoleV2};

let req = RequestV2 {
    id: "vision-1".into(),
    attachments: vec![
        // width * height * 3 interleaved RGB octets, no alpha.
        Attachment::Image { id: "img-1".into(), width, height, bytes: rgb },
        // Mono little-endian float32 PCM at the *advertised* rate.
        Attachment::Audio { id: "clip-1".into(), sample_rate: rate, bytes: pcm },
    ],
    messages: vec![MessageV2 {
        role: RoleV2::User,
        content: vec![
            ContentBlock::Image { attachment_id: "img-1".into() },
            ContentBlock::Audio { attachment_id: "clip-1".into() },
            ContentBlock::Text { text: "Describe the image, then transcribe the clip.".into() },
        ],
    }],
    ..Default::default()
};
```

**Audio: read the sample rate, don't hardcode it.** The daemon rejects
an `Attachment::Audio` whose `sample_rate` differs from what the loaded
backend requires, and never resamples — libmtmd's audio entry point
takes no rate argument, so a wrong rate isn't a detectable error, it
time-scales the clip and returns a fluent *wrong* answer. Get the rate
from the admin socket's `capabilities` event:

```rust,ignore
// AdminEvent { status: "capabilities", audio_sample_rate: Some(16000), .. }
let rate = event.audio_sample_rate.expect("backend advertises no audio rate");
```

Re-read it after a daemon restart; a restart can land on a different
projector. Resampling is the consumer's job — `inferd-http` is the
reference implementation ([ADR 0025](https://github.com/3rg0n/inferd/blob/main/docs/adr/0025-bridge-decodes-and-resamples-audio.md)),
so point an OpenAI SDK at the bridge if you'd rather not own conversion.

Per-request bounds: 32 attachments and 128 MiB aggregate, separate from
the 64 MiB single-frame cap.

## Rerank (cross-encoder reordering)

`RerankClient` is a single round-trip like `EmbedClient`, but the model
is a cross-encoder: query and document are scored *together*, one
forward pass per document, so nothing is precomputable. That is why it
belongs **downstream of retrieval** — `embed → vector search → top-50 →
rerank → top-5 → generate` — not as a replacement for it.

```rust,ignore
use inferd_client::{RerankClient, RerankRequest, RerankResponse};

let mut client = RerankClient::dial_uds(&inferd_client::default_rerank_addr()).await?;
let resp = client
    .rerank(RerankRequest {
        id: "rr-1".into(),
        query: "how do I bind a unix socket".into(),
        documents: candidates.clone(),
        top_n: Some(5),
    })
    .await?;

match resp {
    // Already sorted by score descending and truncated to `top_n`.
    RerankResponse::Rerank { results, .. } => {
        for r in results {
            println!("{:.3}  {}", r.score, candidates[r.index as usize]);
        }
    }
    RerankResponse::Error { code, message, .. } => eprintln!("[{code:?}] {message}"),
}
```

Three contracts worth internalising:

- **The response carries indices, not text.** `results[i].index` is an
  offset into the `documents` you sent; resolve it against your own
  candidate list. Ties keep input order (stable sort).
- **`score` is a raw logit.** Not normalised, not a probability,
  negative values ordinary, scale model-specific — ordinal *within one
  response* only. Don't persist one or compare two.
- **Bounds are on count, not just bytes**: 256 documents and 8 MiB
  aggregate, enforced at parse time. The 64 MiB frame cap bounds bytes,
  and rerank's cost is per *document*.

The socket is bound only when the warm model has a classification head —
a cross-encoder GGUF such as `bge-reranker-v2-m3`. Gemma 4 and
EmbeddingGemma do not serve rerank, and pointing a rerank-enabled daemon
at either fails the load rather than returning meaningless scores.

## Transports

| Constructor | Platform |
|---|---|
| `ClientV2::dial_uds(&path)` | Unix |
| `ClientV2::dial_pipe(r"\\.\pipe\inferd")` | Windows |

`default_v2_addr()` returns the platform default generation socket path.
For embeddings use `EmbedClient::dial_*` (ADR 0017); for rerank
`RerankClient::dial_*` with `default_rerank_addr()` (ADR 0027).

> The daemon binds no inbound network listener — it is reachable only
> over the local UDS / named pipe ([ADR 0022](https://github.com/3rg0n/inferd/blob/main/docs/adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md)).
> The `dial_tcp` constructor was removed in 0.5.0 (deprecated in 0.4.0);
> reach inferd over a network port via the separate `inferd-http` bridge
> ([ADR 0020](https://github.com/3rg0n/inferd/blob/main/docs/adr/0020-inferd-http-bridge-is-a-separate-process.md)).

## Wait-for-ready

Two patterns:

- **Pattern A — passive**: `dial_and_wait_ready(timeout, dial_fn)`.
  Retries connect with exponential backoff (100ms → 5s cap) for
  transient errors during daemon bring-up. Permanent errors
  (permission denied, malformed addr) bubble up immediately.
  Recommended for inference-only consumers.
- **Pattern B — active**: `AdminClient` subscribes to the admin
  socket and yields lifecycle events
  (`starting`/`loading_model`/`ready`/`restarting`/`draining`).
  Use this for installer GUIs, dashboards, or middleware that
  wants progress UX during first-boot model download.

## Daemon endpoints (default paths)

| Platform | Generation | Embed | Rerank | Admin |
|---|---|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/inferd.sock` | `…/infer.embed.sock` | `…/infer.rerank.sock` | `…/admin.sock` |
| macOS | `${TMPDIR}/inferd/inferd.sock` | `…/infer.embed.sock` | `…/infer.rerank.sock` | `…/admin.sock` |
| Windows | `\\.\pipe\inferd` | `\\.\pipe\inferd-infer-embed` | `\\.\pipe\inferd-infer-rerank` | `\\.\pipe\inferd-admin` |

Operators may override via `--uds` / `--pipe` / `--admin-addr` on the
daemon. Each inference socket is bound only when the configured backend
advertises the matching capability, so which of the three exist tells you
what the warm model can do. The daemon binds no inbound network listener
(ADR 0022); network access is the `inferd-http` bridge's job (ADR 0020).

## Versioning

Pinned to the same major/minor as `inferd-proto` (this crate
re-exports the wire types). Cargo's lock-file is the version-pin
contract:

```toml
[dependencies]
inferd-client = "0.6"
```

`inferd-client 0.6.x` always uses `inferd-proto 0.6.x` and talks
to `inferd-daemon 0.6.x`. The published patch versions move in
lockstep. The generation (v2), embed, and rerank surfaces are each
frozen: changes within a surface are backwards-additive only; a breaking
change to the generation wire bumps the in-band `wire_version`
(ADR 0021), so a mismatch fails loudly rather than corrupting the
stream. v0.6, v0.5, and v0.4 are all wire-compatible (backwards-additive
changes only; the `response_format` grammar field in 0.5.0 and the
`thinking` reasoning-activation field in 0.5.1 are optional). The v0.4 → v0.3 framing change
*was* breaking — a v0.3 client does not interoperate with a v0.4+
daemon; upgrade both together.

## Compatibility

End-to-end tested against the live `inferd-daemon` binary:
[`crates/inferd-daemon/tests/echo.rs`](https://github.com/3rg0n/inferd/blob/main/crates/inferd-daemon/tests/echo.rs).
The Go sibling client at `clients/go/` follows the same wire
contract.

## License

MIT. See `LICENSE`.

## Contributing

Bug reports, design discussions, and PRs welcome at
[github.com/3rg0n/inferd](https://github.com/3rg0n/inferd). Read
`CONTRIBUTING.md` in the upstream repo before opening a PR.
