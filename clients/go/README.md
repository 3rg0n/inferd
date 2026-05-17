# inferd-go

Go client for the inferd daemon. Submodule of this monorepo at
`github.com/3rg0n/inferd/clients/go`.

**Status: M5 implemented.** Hand-written translation of `inferd-proto`,
plus a `Client` wrapper that connects over loopback TCP, Unix
domain socket (Unix), or Windows named pipe.

```go
import inferd "github.com/3rg0n/inferd/clients/go"

ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
defer cancel()

client, err := inferd.DialTCP(ctx, "127.0.0.1:47321")
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

## Transport

- `DialTCP(ctx, "127.0.0.1:47321")` — anywhere.
- `DialUDS(ctx, "/run/inferd/infer.sock")` — Unix only (`//go:build unix`).
- `DialPipe(ctx, "\\\\.\\pipe\\inferd-infer")` — Windows only.

## Compatibility

The wire format is frozen per ADR 0008 in the upstream repo. This module
implements protocol v1: the request/response shapes in `protocol.go` are
byte-compatible with `docs/protocol-v1.md` and verified by an end-to-end
test that launches the Rust daemon binary with the mock backend and
round-trips one request through it.

`thlibo v0.2` consumes this module to retire its embedded daemon —
delete `internal/daemon/` and `internal/ipc/`, import this module,
update call sites to construct `inferd.Client`.

## Tests

```sh
# Protocol shape only (no daemon):
go test ./...

# End-to-end against the real daemon (requires `cargo build -p inferd-daemon`):
go test ./...
```

The end-to-end test auto-detects the daemon at `<workspace>/target/debug/
inferd-daemon[.exe]`. Override with `INFERD_DAEMON_BIN=/abs/path`.
