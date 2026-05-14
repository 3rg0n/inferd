# inferd wire protocol v1

This is the authoritative reference for the bytes on the wire.
**v1 is byte-compatible with thlibo v0.1's IPC protocol** so a
thlibo v0.2 client can talk to an inferd v0.1 daemon without any
marshalling changes. When writing the Rust implementation, defer to
thlibo's `internal/ipc/protocol.go` for any ambiguity.

## Framing

- Each frame is a single JSON object, UTF-8 encoded, terminated by a
  single `\n` byte.
- No pretty-printing. No embedded raw newlines inside strings —
  `serde_json`'s default is correct.
- Each direction is an independent NDJSON stream (request stream
  from client, response stream from daemon).
- Maximum frame size: **64 MiB**. Exceeding the limit closes the
  connection and logs `request_oversized`.

## Request

```json
{
  "id": "string, caller-assigned",
  "messages": [
    {"role": "system",    "content": "..."},
    {"role": "user",      "content": "..."},
    {"role": "assistant", "content": "..."}
  ],
  "temperature": 1.0,
  "top_p":       0.95,
  "top_k":       64,
  "max_tokens":  1000,
  "stream":      true,
  "grammar":     "<GBNF string, optional>"
}
```

- `id` is echoed on every response frame so clients can fan-in
  multiple in-flight requests on the same connection.
- `role` is one of `system`, `user`, `assistant`.
- Sampling fields are required. Defaults from Gemma 4 are
  `temperature=1.0, top_p=0.95, top_k=64` (reference: thlibo spec §
  "Gemma 4 E4B reference").
- `stream: false` returns a single `Response::Done` frame with the
  complete generation in a dedicated `text` field (v1 does not
  require this; servers MAY treat it as always-true).
- `grammar`, if present, is passed through to the backend. For
  llamafile, GBNF.

Image content (v1 carries this forward even though the llamafile
adapter doesn't yet use it):

```json
{"role": "user", "content": [
  {"type": "text",  "text": "..."},
  {"type": "image", "data": "<base64>", "budget": 280}
]}
```

- `budget` must be one of `{70, 140, 280, 560, 1120}` and must
  appear before any `text` part for the same message.

## Response stream

Every response frame carries a `type` discriminator:

```json
{"type": "token",  "id": "...", "text": "partial"}
{"type": "done",   "id": "...", "text": "<full text if stream=false>", "stop_reason": "end|length|cancelled"}
{"type": "error",  "id": "...", "message": "human readable"}
{"type": "status", "id": "...", "status": "ready|restarting|draining"}
```

The stream for a single request id ends with exactly one `done` or
one `error` frame. Clients treat any other termination (EOF without
a terminal frame) as an error and invoke their fallback path.

## Admission semantics

- 1 active generation, 10 queued (configurable), non-blocking
  `Submit`. Queue full returns `{"type":"error","message":"queue full"}`
  immediately.
- Client disconnect cancels the in-flight job. `stop_reason` on the
  emitted `done` frame (if the daemon bothers to emit one before the
  socket is closed) is `cancelled`.

## Transport

- Unix domain socket at `/run/inferd/infer.sock` (Linux) or
  `/var/run/inferd/infer.sock` (macOS). Mode `0660`, group
  `inferd-users`.
- Windows named pipe `\\.\pipe\inferd-infer`. SDDL grants the current
  user SID only.
- Loopback TCP `127.0.0.1:47321` (note: different default port from
  thlibo's 47320 to allow side-by-side operation during migration),
  optional `X-Inferd-Key` header-style first frame for API-key auth
  when the socket is exposed over TCP.

## Ready gating

The daemon's socket/pipe is **not created** until the backend emits a
`ready` status internally (llamafile's `READY` on stderr). Clients
that fail to connect during boot should treat the absence of the
socket as "daemon not ready yet", not as an error.

## What changed from thlibo v0.1

Nothing in terms of bytes on the wire. The rename of "thlibo" →
"inferd" shows up only in default paths, group names, and environment
variables (`THLIBO_*` → `INFERD_*`).
