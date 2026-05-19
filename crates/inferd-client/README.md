# inferd-client

Rust client for the [inferd](https://github.com/3rg0n/inferd)
local-inference daemon.

NDJSON-over-IPC. Wire protocol frozen as v1; full spec at
[`docs/protocol-v1.md`](https://github.com/3rg0n/inferd/blob/main/docs/protocol-v1.md)
in the upstream repo.

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
use inferd_client::{Client, Request, Message, Role, Response};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pattern A: connect-and-retry against the inference socket.
    // The successful connect IS the readiness signal — F-13 in the
    // upstream threat model guarantees the inference socket only
    // exists when the daemon is `ready`.
    let mut client = inferd_client::dial_and_wait_ready(
        std::time::Duration::from_secs(30),
        || Client::dial_tcp("127.0.0.1:47321"),
    )
    .await?;

    let mut stream = client
        .generate(Request {
            id: "demo-1".into(),
            messages: vec![Message {
                role: Role::User,
                content: "hello".into(),
            }],
            ..Default::default()
        })
        .await?;

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

## Transports

| Constructor | Platform |
|---|---|
| `Client::dial_tcp("127.0.0.1:47321")` | All |
| `Client::dial_uds(&path)` | Unix |
| `Client::dial_pipe(r"\\.\pipe\inferd-infer")` | Windows |

## Wait-for-ready

Two patterns from the upstream `docs/protocol-v1.md` §"Client
connection lifecycle":

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

| Platform | Inference | Admin |
|---|---|---|
| Linux | `${XDG_RUNTIME_DIR}/inferd/infer.sock` | `${XDG_RUNTIME_DIR}/inferd/admin.sock` |
| macOS | `${TMPDIR}/inferd/infer.sock` | `${TMPDIR}/inferd/admin.sock` |
| Windows | `\\.\pipe\inferd-infer` | `\\.\pipe\inferd-admin` |

Operators may override via `--uds` / `--pipe` / `--admin-addr` on
the daemon. Loopback TCP (`127.0.0.1:47321`) is opt-in for
container / WSL scenarios and supports an API key as the first
NDJSON frame.

## Versioning

Pinned to the same major/minor as `inferd-proto` (this crate
re-exports the wire types). Cargo's lock-file is the version-pin
contract:

```toml
[dependencies]
inferd-client = "0.1"
```

`inferd-client 0.1.x` always uses `inferd-proto 0.1.x` and talks
to `inferd-daemon 0.1.x`. The published patch versions move in
lockstep; the crate-level `=`-pin keeps the wire contract exact
across the workspace at build time. Upstream protocol-v1 changes
are backwards-additive only; breaking changes go to v2 on a
separate socket.

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
