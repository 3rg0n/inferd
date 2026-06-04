# inferd-go

Go client for the inferd daemon. Submodule of this monorepo at
`github.com/3rg0n/inferd/clients/go`.

```go
import inferd "github.com/3rg0n/inferd/clients/go"
```

Single flat package — same shape as `lib/pq` / `pgx`. No
`proto/v1` + `client` subdivision.

Covers all three frozen wire surfaces: v1 text generation
(`Client.Generate`), v2 typed content blocks / attachments / tools
(`Client.GenerateV2`, ADR 0015), and the admin lifecycle stream
(`AdminClient`). v2 is what you use for **multimodal** — sending images
to a vision-capable daemon.

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

## Multimodal (v2)

The v2 surface binds on a **separate socket** from v1 — dial it with
the same `DialUDS` / `DialPipe` / `DialTCP` pointed at the v2 path
(`DefaultInferV2Addr()` returns the platform default), then call
`GenerateV2`. Send images as typed content blocks plus a top-level
attachment table; per ADR 0016 the consumer decodes the image to raw
interleaved RGB (`width*height*3` bytes, no alpha) before sending — the
daemon links no image codec.

```go
// Gate on the daemon advertising vision before dispatching. The admin
// stream emits one capabilities frame per backend.
admin, _ := inferd.DialAdmin(ctx, "")
defer admin.Close()
vision := false
for i := 0; i < 8; i++ {
    ev, err := admin.Recv(ctx)
    if err != nil { break }
    if ev.IsCapabilities() && ev.SupportsVision() { vision = true; break }
    if ev.Status == "ready" { break }
}
if !vision { /* daemon has no vision backend; fall back to text */ }

// Decode your image (JPEG/PNG/…) to RGB yourself, then:
rgb := decodeToRGB(imgBytes)         // your codec; daemon links none
c, _ := inferd.DialUDS(ctx, inferd.DefaultInferV2Addr())
defer c.Close()
stream, _ := c.GenerateV2(ctx, inferd.RequestV2{
    ID: "vq-1",
    Messages: []inferd.MessageV2{{
        Role: inferd.RoleUser,
        Content: []inferd.ContentBlock{
            inferd.TextBlock("What's in this image?"),
            inferd.ImageBlock("img"),
        },
    }},
    Attachments: []inferd.AttachmentV2{
        inferd.ImageAttachment("img", w, h, rgb),
    },
})
for f := range stream {
    if f.Type == inferd.ResponseV2Frame && f.Block != nil && f.Block.Type == inferd.BlockText {
        fmt.Print(f.Block.Delta)
    }
}
```

Streaming text arrives as `frame` responses carrying a `text` block
delta; `thinking` blocks carry the reasoning trace separately; a
complete `tool_use` block arrives whole when the model calls a tool
declared in `RequestV2.Tools`. The stream terminates with one `done`
(carrying `UsageV2` + `StopReasonV2`) or one `error`.

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

Each wire surface is frozen in the upstream repo: v1 per ADR 0008,
v2 per ADR 0015, embeddings per ADR 0017. This module implements v1
(`protocol.go`), v2 (`protocol_v2.go`), and the admin stream
(`admin.go`); the shapes are byte-compatible with the Rust
`inferd-proto` crate and verified by tests that round-trip frames
(and, when the binary is present, launch the Rust daemon).

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
