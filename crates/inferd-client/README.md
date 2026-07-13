# inferd-client

Rust client for the [inferd](https://github.com/3rg0n/inferd)
local-inference daemon.

Two frozen wire surfaces, each on its own socket: generation
(`ClientV2` — typed content blocks / attachments / tools, ADR 0015)
on the length-prefixed, type-tagged framing introduced in v0.4
([ADR 0021](https://github.com/3rg0n/inferd/blob/main/docs/adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)),
and embeddings (`EmbedClient`, ADR 0017, NDJSON). The original
text-only v1 surface was folded into `ClientV2` and removed in v0.4.

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

## Transports

| Constructor | Platform |
|---|---|
| `ClientV2::dial_uds(&path)` | Unix |
| `ClientV2::dial_pipe(r"\\.\pipe\inferd")` | Windows |

`default_v2_addr()` returns the platform default generation socket path.
For embeddings, use `EmbedClient::dial_*` (ADR 0017).

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

| Platform | Generation | Admin |
|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/inferd.sock` | `${XDG_RUNTIME_DIR}/inferd/admin.sock` |
| macOS | `${TMPDIR}/inferd/inferd.sock` | `${TMPDIR}/inferd/admin.sock` |
| Windows | `\\.\pipe\inferd` | `\\.\pipe\inferd-admin` |

Operators may override via `--uds` / `--pipe` / `--admin-addr` on
the daemon. (The embed surface binds its own socket when an
embed-capable backend is configured.) The daemon binds no inbound
network listener (ADR 0022); network access is the `inferd-http`
bridge's job (ADR 0020).

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
lockstep. The generation (v2) and embed surfaces are each frozen:
changes within a surface are backwards-additive only; a breaking
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
