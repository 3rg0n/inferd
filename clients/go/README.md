# inferd-go

Go client for the inferd daemon. Submodule of this monorepo at
`github.com/3rg0n/inferd/clients/go`.

```go
import inferd "github.com/3rg0n/inferd/clients/go"
```

Single flat package — same shape as `lib/pq` / `pgx`. No
`proto/v1` + `client` subdivision. Stdlib-only: no third-party
dependencies.

## Versioning

This is a **nested Go module** in a monorepo subdirectory, so its
versions are published as **path-prefixed tags** (`clients/go/vX.Y.Z`),
not the repo's root `vX.Y.Z` tags. Pin it the normal way:

```sh
go get github.com/3rg0n/inferd/clients/go@v0.6.1
```

Go maps that to the `clients/go/v0.6.1` tag automatically. The
client versions in **lockstep** with the daemon — `clients/go/<v>` is
cut at the same commit as the root `<v>` release, so the client version
you pin is the wire/daemon version it was built against.

Covers both frozen wire surfaces: generation (`Client.GenerateV2` —
typed content blocks / attachments / tools, ADR 0015) and the admin
lifecycle stream (`AdminClient`). Generation is what you use for
**multimodal** — sending images to a vision-capable daemon. The
original text-only v1 surface was folded into `GenerateV2` and removed
in v0.4; a text-only turn is a single `TextBlock`.

## Quickstart

```go
ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
defer cancel()

// Pattern A: connect-and-retry against the generation socket.
// The successful connect IS the readiness signal — F-13 in the
// upstream threat model guarantees the generation socket only
// exists when the daemon is `ready`.
client, err := inferd.DialAndWaitReady(ctx, func(ctx context.Context) (*inferd.Client, error) {
    return inferd.DialInfer(ctx) // platform default: UDS on Unix, named pipe on Windows
})
if err != nil { /* handle */ }
defer client.Close()

// Text-only is a single TextBlock; GenerateV2 stamps wire_version.
stream, err := client.GenerateV2(ctx, inferd.RequestV2{
    ID: "demo-1",
    Messages: []inferd.MessageV2{
        {Role: inferd.RoleUser, Content: []inferd.ContentBlock{inferd.TextBlock("hello")}},
    },
})
if err != nil { /* handle */ }

for frame := range stream {
    switch frame.Type {
    case inferd.ResponseV2Frame:
        if frame.Block != nil && frame.Block.Type == inferd.BlockText {
            fmt.Print(frame.Block.Delta)
        }
    case inferd.ResponseV2Done:
        fmt.Printf("\n[done; backend=%s, stop=%s]\n", frame.Backend, frame.StopReason)
    case inferd.ResponseV2Error:
        fmt.Printf("\n[error %s: %s]\n", frame.Code, frame.Message)
    }
}
```

## Multimodal

Generation is a single socket (v0.4 / ADR 0021) — dial it with
`DialUDS` / `DialPipe` / `DialTCP` pointed at `DefaultInferAddr()`,
then call `GenerateV2`. Send images as typed content blocks plus a
top-level attachment table; per ADR 0016 the consumer decodes the
image to raw interleaved RGB (`width*height*3` bytes, no alpha) before
sending — the daemon links no image codec. The raw bytes ride as a
BLOB frame keyed by attachment id (ADR 0021); the client emits the
frames for you.

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
c, _ := inferd.DialUDS(ctx, inferd.DefaultInferAddr())
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

### Audio: the sample rate is a hard contract

Audio works the same way — `AudioBlock` plus `AudioAttachment` carrying
little-endian float32 PCM (mono, 4 bytes per sample), decoded by you
per ADR 0016 — with one extra rule that has no analogue on the image
path.

**The sample rate you declare must equal the rate the backend
advertises.** The model's audio encoder takes no rate parameter: it
consumes samples at whatever rate it was trained for. Handing a 16 kHz
encoder 44.1 kHz audio time-scales it ~2.75x and yields a confident,
fluent, *wrong* answer — nothing in the bytes reveals the error. So the
daemon rejects a mismatch with `invalid_request` naming both rates
rather than resampling. Read the required rate off the capabilities
frame; don't hardcode it.

```go
rate, ok := ev.RequiredAudioSampleRate()   // ev.SupportsAudio() first
if !ok { /* backend advertises audio but no rate; don't guess */ }

pcm := decodeToF32LE(audioBytes, rate)     // your codec + resampler
stream, _ := c.GenerateV2(ctx, inferd.RequestV2{
    ID: "aq-1",
    Messages: []inferd.MessageV2{{
        Role: inferd.RoleUser,
        Content: []inferd.ContentBlock{
            inferd.AudioBlock("clip"),
            inferd.TextBlock("Transcribe this audio verbatim."),
        },
    }},
    Attachments: []inferd.AttachmentV2{
        inferd.AudioAttachment("clip", rate, pcm),
    },
})
```

Gemma-4-class encoders advertise 16000, but read the value — a future
model will not.

Streaming text arrives as `frame` responses carrying a `text` block
delta; `thinking` blocks carry the reasoning trace separately; a
complete `tool_use` block arrives whole when the model calls a tool
declared in `RequestV2.Tools`. The stream terminates with one `done`
(carrying `UsageV2` + `StopReasonV2`) or one `error`.

`RequestV2.ToolChoice` constrains tool use: `ToolChoiceAuto`,
`ToolChoiceRequired`, `ToolChoiceNone`. It is a constraint rather than a
hint — on a backend that enforces it, `ToolChoiceRequired` cannot come
back as text ([ADR 0029](https://github.com/3rg0n/inferd/blob/main/docs/adr/0029-tool-choice-is-enforced-by-grammar-not-advertised.md)).
The daemon rejects it without a non-empty `Tools`, and rejects it
alongside `ResponseFormat`, since only one decoding constraint can be
installed. Leave it empty to omit the field.

`ToolChoiceRequired` bounds where the turn may *end*, not what it
contains: a model that disagrees with the prompt can decline for its
whole budget and stop at `StopMaxTokens`. When that happens the `done`
frame carries `ToolChoiceUnsatisfied: true`, so branch on that rather
than on `StopReason` — `StopMaxTokens` also means "ran out of room
mid-answer", and the stop reason alone cannot separate the two:

```go
if frame.Type == inferd.ResponseV2Done && frame.ToolChoiceUnsatisfied {
    // No call arrived. Retry with a different prompt, fall back, or
    // surface the refusal — the daemon does not retry (ADR 0007).
}
```

The field is absent (false) on every other request, including one that
never sent a `ToolChoice`.

## Transports

| Function | Platform | Default (`DefaultInferAddr()`) |
|---|---|---|
| `DialInfer(ctx)` | All | Platform default: UDS on Unix, named pipe on Windows. |
| `DialUDS(ctx, path)` | Unix (`//go:build unix`) | `${XDG_RUNTIME_DIR}/inferd/inferd.sock` |
| `DialPipe(ctx, path)` | Windows | `\\.\pipe\inferd` |

> The daemon binds no inbound network listener — it is reachable only
> over the local UDS / named pipe ([ADR 0022](https://github.com/3rg0n/inferd/blob/main/docs/adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md)).
> The `DialTCP` function was removed in 0.5.0 (deprecated in 0.4.0); for
> network access use the separate `inferd-http` bridge (ADR 0020).

`DialAndWaitReady(ctx, dial)` wraps any of the three with an
exponential-backoff retry loop (start 100ms, cap 5s) for
transient connect errors that surface during daemon bring-up
(`ECONNREFUSED`, `ENOENT`, `ERROR_PIPE_BUSY`,
`ERROR_FILE_NOT_FOUND`). Permanent errors (`EACCES`, malformed
addr) bubble up immediately. Use this for inference-only
consumers (Pattern A).

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
    return inferd.DialInfer(c)
})
defer client.Close()
// ... use client.Generate as in the quickstart.
```

`AdminClient.WaitReady(ctx)` is a one-call helper that loops
`Recv` until it sees a `ready` event, useful when you don't
need the progress frames in between.

### Forward compatibility

Per the spec, clients **MUST ignore** unknown `Status` and
`Phase` values; the daemon may add new ones in any release.
This client surfaces unknown values verbatim in
`AdminEvent.Status`/`AdminEvent.Phase` — branch only on values
you recognise; default to logging-and-ignoring otherwise.

## Compatibility

Each wire surface is frozen in the upstream repo: generation (v2) per
ADR 0015 with the length-prefixed framing of ADR 0021, embeddings per
ADR 0017, rerank per ADR 0027. This module implements the generation wire
(`protocol_v2.go` / `client_v2.go`) and the admin stream (`admin.go`);
the shapes are byte-compatible with the Rust `inferd-proto` crate and
verified by tests that round-trip frames (and, when the binary is
present, launch the Rust daemon). The text-only v1 surface was removed
in v0.4.

The embed and rerank NDJSON surfaces have no Go client yet — this module
carries their default socket paths (`DefaultInferEmbedAddr`,
`DefaultInferRerankAddr`) and their capability flags on the admin stream
(`AdminEvent.Embed`, `AdminEvent.SupportsRerank`), so a consumer can
discover them and speak NDJSON directly. Neither flag implies the other:
rerank needs a classification head that a bi-encoder embedding model
does not have.

Any Go consumer that wants local inference imports this
module instead of embedding its own engine. Call sites
construct `inferd.Client`, point it at the running daemon's
endpoint (UDS on Unix, named pipe on Windows), and stream
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
