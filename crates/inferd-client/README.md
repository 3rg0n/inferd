# inferd-client

Rust client for the [inferd](https://github.com/3rg0n/inferd)
local-inference daemon.

NDJSON-over-IPC. Wire protocol frozen as v1; full spec at
[`docs/protocol-v1.md`](https://github.com/3rg0n/inferd/blob/main/docs/protocol-v1.md)
in the upstream repo.

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

## Versioning

Pinned to the same major/minor as `inferd-proto` (this crate
re-exports the wire types). Cargo's lock-file is the version-pin
contract:

```toml
[dependencies]
inferd-client = "0.1"
```

`inferd-client 0.1` always uses `inferd-proto 0.1` and talks to
`inferd-daemon 0.1`. Upstream protocol-v1 changes are
backwards-additive only; breaking changes go to v2 on a separate
socket.

## Compatibility

End-to-end tested against the live `inferd-daemon` binary:
[`crates/inferd-daemon/tests/echo.rs`](https://github.com/3rg0n/inferd/blob/main/crates/inferd-daemon/tests/echo.rs).
The Go sibling client at `clients/go/` follows the same wire
contract.
