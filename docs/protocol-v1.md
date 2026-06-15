# inferd wire protocol v1 — HISTORICAL

> **This document is a historical record.** Protocol v1 — the text-only
> NDJSON generation wire described below — was folded into v2 and
> **removed in v0.4** ([ADR 0021](adr/0021-unified-v2-wire-length-prefixed-blob-framing.md)).
> A v0.4 daemon does not bind a v1 socket and the v1 proto types are
> gone. The **live** generation surface is the v2 wire (typed content
> blocks, ADR 0015) on length-prefixed, type-tagged framing with an
> in-band `wire_version` (ADR 0021); embeddings are on their own socket
> (ADR 0017). This page is kept for reference on v0.3-and-earlier
> deployments and for the design rationale that still informs v2.

This was the authoritative reference for the bytes on the wire
between an inferd client and the inferd daemon through v0.3. v1 was
designed for inferd on its own merits (see ADR 0008). It is *not*
derived from any other project's protocol.

## Framing

- Each frame is a single JSON object, UTF-8 encoded, terminated
  by a single `\n` byte.
- No pretty-printing. No embedded raw newlines inside strings.
- Each direction is an independent NDJSON stream (request
  stream from client, response stream from daemon).
- Maximum frame size: **64 MiB**. Exceeding the limit returns
  an `error` frame with `code: "frame_too_large"` and closes
  the connection. The daemon logs the event as
  `request_oversized`.

## Request

```json
{
  "id":                 "string, caller-assigned",
  "messages": [
    {"role": "system",    "content": "..."},
    {"role": "user",      "content": "..."},
    {"role": "assistant", "content": "..."}
  ],
  "temperature":        1.0,
  "top_p":              0.95,
  "top_k":              64,
  "max_tokens":         1000,
  "stream":             true,
  "image_token_budget": 280,
  "grammar":            "<GBNF string, optional>"
}
```

- `id` is echoed on every response frame so clients can fan in
  multiple in-flight requests on the same connection. May be
  omitted on the wire; the daemon echoes whatever arrives.
  Callers should send a non-empty id for correlation.
- `role` is one of `system`, `user`, `assistant`. `content` is
  a UTF-8 string. v1 does not support multimodal content
  arrays — see "Image content" below.
- Sampling fields are **all optional**. Omitted fields receive
  Gemma 4 defaults: `temperature=1.0`, `top_p=0.95`, `top_k=64`,
  `max_tokens=1000`, `stream=true`. Defaults are applied
  server-side after parse.
- `image_token_budget`, if present, must be one of
  `{70, 140, 280, 560, 1120}`. Any other value returns an
  `error` frame with `code: "invalid_request"` before the
  request reaches the backend. v1 carries this field forward;
  v0.1 backends do not consume image content (text-only
  generation).
- `grammar`, if present, is a llama.cpp GBNF string forwarded
  verbatim to the backend. Empty/omitted = unconstrained.
- The wire protocol exposes **no** field for selecting a
  backend. Apps do not pick the backend; the daemon's router
  decides per ADR 0007.

### Image content

v1 declares image budgets via the top-level
`image_token_budget` field. v1 does **not** ship a multimodal
`content` array shape; that is reserved for v2. Backends that
do not support image input ignore `image_token_budget`.

## Response stream

Every response frame is a single JSON object with a `type`
discriminator. Field set by frame type:

```json
{"id": "...", "type": "status", "status": "loading_model|ready|restarting|draining"}
{"id": "...", "type": "token",  "content": "partial text"}
{"id": "...", "type": "done",   "content": "<full text>", "usage": {"prompt_tokens": N, "completion_tokens": M}, "stop_reason": "end|length|cancelled|error", "backend": "llamacpp"}
{"id": "...", "type": "error",  "code": "queue_full|backend_unavailable|invalid_request|frame_too_large|internal", "message": "human readable"}
```

Field guarantees:

- `id` is present on every frame. For status frames not tied
  to a specific request (e.g. the startup `loading_model` →
  `ready` transition broadcast on the admin socket), the
  literal id `"admin"` is used.
- `content` carries token text on `token` frames, and the
  complete generated text on `done` frames.
- `usage` is present on `done` frames and reports
  `prompt_tokens` and `completion_tokens` integer counts.
- `stop_reason` is present on `done` frames and is one of:
  - `end` — model emitted the end-of-turn token cleanly.
  - `length` — `max_tokens` reached.
  - `cancelled` — caller disconnected or otherwise cancelled.
  - `error` — generation aborted; an `error` frame may have
    followed instead of a `done` frame, but if a `done` frame
    is emitted the partial output is in `content` and
    `stop_reason` is `error`.
- `backend` is present on `done` frames and names the
  `Backend::name()` that served the request — e.g. `llamacpp`,
  `mock`, or in v0.2 `anthropic`, `bedrock`. Diagnostic only;
  app logic must not branch on backend identity.
- `code` is present on `error` frames and is one of:
  - `queue_full` — admission queue full at submit time. Caller
    may retry immediately or with backoff.
  - `backend_unavailable` — selected backend errored before
    or during generation. Caller may retry; the router may
    pick a different backend on the retry.
  - `invalid_request` — request failed validation (bad role,
    invalid `image_token_budget`, malformed JSON). Caller
    should not retry without changing the request.
  - `frame_too_large` — frame exceeded the 64 MiB cap.
    Connection is closed.
  - `internal` — daemon-side bug. Caller may retry with
    backoff but should not assume retry will succeed.
- `message` is present on `error` frames and is a
  human-readable description.
- The stream for a single request id ends with exactly one
  `done` or one `error` frame. Clients treat any other
  termination (EOF without a terminal frame) as an error and
  invoke their fallback path.

## Admission semantics

- 1 active generation, 10 queued (configurable via daemon
  config, **not** the wire). Non-blocking submit. Queue full
  returns an `error` frame with `code: "queue_full"`
  immediately and closes the request stream.
- Client disconnect cancels the in-flight job. The daemon may
  emit a `done` frame with `stop_reason: "cancelled"` if it
  can do so before the socket is fully closed; if not, the
  caller learns of cancellation by the EOF.
- The daemon does not retry. Backend failures emit one
  `error` frame and end the stream; caller owns retry policy.
  See ADR 0007.

## Transport

- **Unix domain socket** at the platform-specific path resolved
  via the algorithm in §"Default endpoint resolution" below.
  Mode `0660`, group `inferd-users`.
- **Windows named pipe** `\\.\pipe\inferd-infer`. ACL grants
  the current user SID only; `Everyone` is denied.
- **Loopback TCP** `127.0.0.1:47321`. Optional API-key auth as
  the first frame on the connection when exposed over TCP.
  Off by default; opt-in for container / WSL scenarios.

The daemon also exposes an **admin endpoint** for push-style
lifecycle event broadcast. Operator tooling, installer GUIs, and
middleware that wants progress UX during first-boot model
download connect here. See §"Admin endpoint" below for the full
contract.

## Admin endpoint

Push-style daemon-lifecycle event broadcast. Read-only NDJSON
stream from daemon → client. Same framing rules as the inference
stream (one JSON object per line, 64 MiB cap).

### Path

| Platform | Path | Permissions |
|---|---|---|
| Linux | resolved via §"Default endpoint resolution" | mode `0600`, daemon uid only |
| macOS | `${TMPDIR}/inferd/admin.sock` | mode `0600`, daemon uid only |
| Windows | `\\.\pipe\inferd-admin` | DACL grants current user SID only |

Configurable via `--admin-addr` / `INFERD_ADMIN_ADDR` for tests
and non-default deployments. Production deployments use the
default. **No TCP admin endpoint** — admin is local-only.

### Default endpoint resolution

To accommodate Linux's `systemd --user` lifecycle (which cannot
write under `/run/<service>/` because that directory is root-only)
the spec freezes a **resolution algorithm** rather than a literal
path. Both the daemon and any compliant client compute the same
path from the same algorithm.

For the inference socket, admin socket, and lock file on Linux,
the path is:

```
${XDG_RUNTIME_DIR}/inferd/<leaf>     if XDG_RUNTIME_DIR is set and non-empty
${HOME}/.inferd/run/<leaf>           else if HOME is set and non-empty
/tmp/inferd-${UID}/<leaf>            else (multi-user-safe last resort)
```

Where `<leaf>` is `infer.sock`, `admin.sock`, or `inferd.lock`
respectively, and `${UID}` is the daemon's effective user id.

`XDG_RUNTIME_DIR` is provisioned per-user by `systemd-logind` on
session start (`/run/user/<uid>/`); the second branch covers
containers / non-logind sessions. The systemd unit at
`packaging/systemd/inferd.service` declares
`RuntimeDirectory=inferd` so step 1 always succeeds when the
unit is run via `systemctl --user`.

On macOS and Windows the path is a single literal (see the
relevant tables) and no resolution is required.

### Lifecycle vs. inference

```
t=0   daemon process starts
t=0+  admin socket bound, accepting connections                 ← clients can connect
t=N   model present + loaded, backend reports ready
t=N+  inference socket bound, accepting connections             ← inference clients can connect
```

The admin socket is **bound first, before any model work**.
Clients can connect immediately and watch lifecycle events
while the daemon is bootstrapping. The inference socket comes
up last, *after* `ready` is published on the admin channel.

### Connect behaviour

When a client connects to the admin socket:

1. The daemon **immediately writes a snapshot frame** carrying
   its current state. A client connecting mid-download gets a
   `loading_model` frame with progress, not a stale earlier
   state.
2. Subsequent state transitions push as they happen.
3. The connection stays open indefinitely. The daemon does not
   close it; the client closes when done.

A client that takes too long reading frames lets the broadcast
queue overflow (256 frames) and gets disconnected (EOF).
Reconnect to resume from the current snapshot.

### Direction

Read-only from the client's perspective. Daemon → client only
in v1. A client that writes to the admin socket gets its bytes
ignored. Future client→server commands (drain, reload) are
reserved for v2.

### Frame envelope

Every admin event:

```json
{
  "id":     "admin",
  "type":   "status",
  "status": "<state>",
  "phase":  "<phase>",
  "...detail keys flattened..."
}
```

- `id` is the literal string `"admin"`.
- `type` is the literal string `"status"`.
- `status` is one of the lifecycle states below.
- `phase` and detail keys (e.g. `downloaded_bytes`, `path`) are
  flattened into the same JSON object — not nested under a
  separate `phase`/`detail` envelope.

### Lifecycle states

| `status` | Meaning |
|---|---|
| `starting` | Daemon process is up; admin socket is bound. No backend work yet. Brief; usually <100ms. |
| `loading_model` | Model is being prepared. Carries `phase` plus detail keys. May take seconds (cached file mmap) or hours (5 GB download on a slow link). |
| `ready` | Inference socket is bound and accepting connections. The daemon is fully usable. |
| `restarting` | Previously-`ready` daemon is reloading. Inference socket is closed; new connections refused. Carries the same `phase` enum as `loading_model`. |
| `draining` | Daemon received a shutdown signal. Existing requests finish; new requests rejected. The daemon will exit shortly after this frame. |

State transitions:

```
starting → loading_model → ready
                ↓             ↓
              (error)    restarting → loading_model → ready
                ↓             ↓             ↓
             draining    draining       draining → exit
```

### `loading_model` phases

When `status: "loading_model"`, the frame includes `phase` plus
phase-specific detail keys.

| `phase` | Detail keys | Meaning |
|---|---|---|
| `checking_local` | `path` | Resolving model path on disk and checking SHA-256. |
| `download` | `downloaded_bytes`, `total_bytes` (may be `null`), `source_url` | Downloading the GGUF. Progress emitted every 32 MiB or every 5 seconds, whichever first. |
| `verify` | `path` | Streaming SHA-256 over downloaded bytes for final verification. |
| `quarantine` | `path`, `expected_sha256`, `actual_sha256`, `quarantine_path` | Downloaded SHA mismatched config; file moved aside. Daemon will retry or refuse per `auto_pull`. |
| `mmap` | `path` | Loading the file into the engine via FFI. |
| `kv_cache` | `n_ctx` | Allocating the KV cache. |

A typical first-boot sequence:

```jsonl
{"id":"admin","type":"status","status":"starting"}
{"id":"admin","type":"status","status":"loading_model","phase":"checking_local","path":"/home/u/.inferd/models/gemma-4-e4b-ud-q4-k-xl.gguf"}
{"id":"admin","type":"status","status":"loading_model","phase":"download","downloaded_bytes":33554432,"total_bytes":5126304928,"source_url":"https://huggingface.co/..."}
{"id":"admin","type":"status","status":"loading_model","phase":"download","downloaded_bytes":67108864,"total_bytes":5126304928,"source_url":"https://huggingface.co/..."}
... (~150 progress frames during a 5 GB download)
{"id":"admin","type":"status","status":"loading_model","phase":"verify","path":"/home/u/.inferd/models/gemma-4-e4b-ud-q4-k-xl.gguf"}
{"id":"admin","type":"status","status":"loading_model","phase":"mmap","path":"/home/u/.inferd/models/gemma-4-e4b-ud-q4-k-xl.gguf"}
{"id":"admin","type":"status","status":"loading_model","phase":"kv_cache","n_ctx":8192}
{"id":"admin","type":"status","status":"ready"}
```

### Forward compatibility

- Clients **MUST ignore** unknown `status` values (display them,
  log them, do not branch on them).
- Clients **MUST ignore** unknown `phase` values within
  `loading_model`.
- Clients **MUST ignore** unknown detail keys.
- The daemon **WILL NOT** introduce a new `status` or `phase`
  that breaks existing semantics. Backwards-additive only;
  breaking changes require v2 on a new socket path.

### Error semantics

The admin socket itself does not emit `error` frames in v1 —
it is a status-broadcast channel; failures of the broadcast
itself are reflected in the connection (EOF) rather than in
protocol frames:

- Daemon crash → admin socket closes, clients see EOF.
- Daemon transitions to `draining` and exits → clients see a
  `draining` frame followed by EOF.
- Slow client → broadcast queue overflows, daemon disconnects
  with EOF. Client reconnects to resume from the current
  snapshot.

## Client connection lifecycle

Two patterns; pick based on whether the client cares about
progress UX.

### Pattern A — passive (recommended for inference-only consumers)

The inference socket only exists when the daemon is `ready`
(THREAT_MODEL F-13). A connect-with-retry loop against the
inference socket *is* the wait-for-ready mechanism — the
successful connect is the ready signal.

```text
loop:
    try connect to inference socket
    if success: break
    if transient error (ECONNREFUSED, ENOENT, pipe-busy):
        sleep with exponential backoff (start 100ms, cap 5s)
        try again
    else:
        bail loudly — that's not transient
```

Recommended cap: 30 seconds for normal startup; longer
(operator-configurable) for first-boot scenarios where a
multi-GB model is downloading. After the cap, surface a clear
error pointing at `systemctl status inferd` /
`sc.exe query inferd-daemon`.

This is the same pattern used by every database client library
(libpq, redis, etcd-client). It works on every platform,
including macOS where the service manager has no native
readiness reporting.

### Pattern B — active (for installer GUIs, dashboards, progress UX)

Connect to the **admin** socket, watch for `status: "ready"`,
then connect to the inference socket. Display progress along
the way using the `loading_model` `phase: download` frames.

```text
connect to admin socket
loop:
    read one NDJSON frame
    if frame.status == "loading_model" and frame.phase == "download":
        update progress bar with frame.downloaded_bytes / frame.total_bytes
    elif frame.status == "ready":
        break
close admin socket
... now connect to inference and send traffic per Pattern A
```

Most middleware uses Pattern A. UI / installer / dashboard
tools use Pattern B.

### What to do during `restarting`

If a client is connected to the admin socket and observes
`restarting`:

- The inference socket has closed. Existing inference
  connections have already received EOF.
- New inference connections will fail with `ECONNREFUSED` /
  `ENOENT` until a subsequent `ready` event.
- Stay connected to the admin socket — wait for `ready`, then
  reconnect on inference per Pattern A.

## Per-caller identity

Each accepted connection is identified by:

- **Unix**: `SO_PEERCRED` (Linux) / `LOCAL_PEERCRED` (macOS) →
  uid + gid + pid.
- **Windows**: `GetNamedPipeClientProcessId` →
  pid + token-derived SID.
- **Loopback TCP**: identity is the API key (if configured) or
  `tcp:<remote-addr>` for log correlation only.

Identity is recorded in the activity log for every request and
used by the admission queue's per-caller counters (when v0.2
adds per-caller fairness).

## Ready gating

The daemon's inference socket/pipe is **not created** until
the configured backend reports ready. Clients that fail to
connect during boot should treat the absence of the socket as
"daemon not ready yet," not as an error.

The admin socket may come up earlier than the inference
socket so that operators can observe the `loading_model` →
`ready` transition.

## Versioning

v1 is immutable. Any breaking change becomes v2, served on a
separate socket path (`infer-v2.sock` / `\\.\pipe\inferd-infer-v2`).
Callers and daemons negotiate by which path they connect to;
there is no in-band capability exchange.

Backwards-additive changes — new optional fields that older
servers MUST ignore and older clients MUST NOT require — are
acceptable on v1 if and only if every existing v1 server in
the wild already ignores unknown fields. v0.1 enforces this
("unknown fields ignored on parse") so the door for additive
changes stays open within v1.
