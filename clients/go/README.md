# inferd-go

Go client for the inferd daemon. Submodule of this monorepo at
`github.com/3rg0n/inferd/clients/go`.

```go
import inferd "github.com/3rg0n/inferd/clients/go"
```

Single flat package — same shape as `lib/pq` / `pgx`. No
`proto/v1` + `client` subdivision in v0.1.

## Quickstart

```go
ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
defer cancel()

// Pattern A: connect-and-retry against the inference socket.
// The successful connect IS the readiness signal — F-13 in the
// upstream threat model guarantees the inference socket only
// exists when the daemon is `ready`.
client, err := inferd.DialAndWaitReady(ctx, func(ctx context.Context) (*inferd.Client, error) {
    return inferd.DialTCP(ctx, "127.0.0.1:47321")
})
if err != nil { /* handle */ }
defer client.Close()

stream, err := client.Generate(ctx, inferd.Request{
    ID: "demo-1",
    Messages: []inferd.Message{
        {Role: inferd.RoleUser, Content: "hello"},
    },
})
if err != nil { /* handle */ }

for frame := range stream {
    switch frame.Type {
    case inferd.ResponseToken:
        fmt.Print(frame.Content)
    case inferd.ResponseDone:
        fmt.Printf("\n[done; backend=%s, stop=%s]\n", frame.Backend, frame.StopReason)
    case inferd.ResponseError:
        fmt.Printf("\n[error %s: %s]\n", frame.Code, frame.Message)
    }
}
```

## Transports

| Function | Platform | Default |
|---|---|---|
| `DialTCP(ctx, "127.0.0.1:47321")` | All | Loopback only by convention; daemon refuses to bind public addresses unless explicitly configured. |
| `DialUDS(ctx, path)` | Unix (`//go:build unix`) | `/run/inferd/infer.sock` |
| `DialPipe(ctx, path)` | Windows | `\\.\pipe\inferd-infer` |

`DialAndWaitReady(ctx, dial)` wraps any of the three with an
exponential-backoff retry loop (start 100ms, cap 5s) for
transient connect errors that surface during daemon bring-up
(`ECONNREFUSED`, `ENOENT`, `ERROR_PIPE_BUSY`,
`ERROR_FILE_NOT_FOUND`). Permanent errors (`EACCES`, malformed
addr) bubble up immediately. Use this for inference-only
consumers — see Pattern A in the upstream
`docs/protocol-v1.md` §"Client connection lifecycle".

## Wait-for-ready patterns

The daemon may take seconds to hours to come up: the first-boot
case downloads a multi-GB GGUF model. There are two patterns
for waiting; pick based on whether you need progress UX.

### Pattern A — passive (recommended)

`DialAndWaitReady` against the inference transport. Successful
connect is the ready signal. No admin-socket plumbing required.
This is the standard Postgres / Redis / etcd client shape.

### Pattern B — active (progress UX)

Connect to the admin socket separately. Watch lifecycle frames.
Display download progress along the way. Then connect to the
inference socket per Pattern A.

```go
admin, err := inferd.DialAdmin(ctx, "")  // "" = platform default
if err != nil { /* handle */ }
defer admin.Close()

for {
    ev, err := admin.Recv(ctx)
    if err != nil { return err }
    switch ev.Status {
    case "loading_model":
        if ev.Phase == "download" {
            total := int64(0)
            if ev.TotalBytes != nil { total = *ev.TotalBytes }
            fmt.Printf("download: %d / %d\n", ev.DownloadedBytes, total)
        }
    case "ready":
        fmt.Println("inferd ready")
        goto inference
    case "draining":
        return errors.New("inferd is shutting down")
    }
}
inference:
client, _ := inferd.DialAndWaitReady(ctx, func(c context.Context) (*inferd.Client, error) {
    return inferd.DialTCP(c, "127.0.0.1:47321")
})
defer client.Close()
// ... use client.Generate as in the quickstart.
```

`AdminClient.WaitReady(ctx)` is a one-call helper that loops
`Recv` until it sees a `ready` event, useful when you don't
need the progress frames in between.

### Forward compatibility

Per the spec, clients **MUST ignore** unknown `Status` and
`Phase` values; the daemon may add new ones in any v1 release.
This client surfaces unknown values verbatim in
`AdminEvent.Status`/`AdminEvent.Phase` — branch only on values
you recognise; default to logging-and-ignoring otherwise.

## Compatibility

The wire format is frozen per ADR 0008 in the upstream repo.
This module implements protocol v1; the request/response
shapes in `protocol.go` and the admin event shapes in
`admin.go` are byte-compatible with `docs/protocol-v1.md` and
verified by tests that launch the Rust daemon binary and
round-trip frames through it.

Any Go consumer that wants local inference imports this
module instead of embedding its own engine. Call sites
construct `inferd.Client`, point it at the running daemon's
endpoint (UDS / named pipe / loopback TCP), and stream
tokens back through the same connection.

## Tests

```sh
# Protocol-shape + admin-event tests (no daemon needed):
go test ./...

# Full suite including end-to-end against the daemon binary
# (requires `cargo build -p inferd-daemon` first):
go test ./...
```

The end-to-end test auto-detects the daemon at
`<workspace>/target/debug/inferd-daemon[.exe]`. Override with
`INFERD_DAEMON_BIN=/abs/path`.
