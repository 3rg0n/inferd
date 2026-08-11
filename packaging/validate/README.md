# install=work validation harness

Stdlib-only Python gates for a per-platform **install=work** leg. Run
these against an *installed* daemon — from a release archive, at real
default paths — and record the results as a row plus a section in
`docs/vX.Y-validation.md`.

Committed rather than kept as scratch so every platform leg runs the
same checks: the first two legs of v0.8.0 each rebuilt this by hand, and
the second one rediscovered two harness bugs the first had already
fixed (see "Traps", below).

## What "install=work" means

A fresh-machine installer → real `generate` + real `embed` + the
OpenAI-compat bridge. No mock backend, no hand-edited config, no dev
build, no "run `pull` first". Anything less is not releasable.

Run the installer at its **real default paths**. Overriding them with
`-BinaryPath` and friends hides exactly the packaging bugs this gate
exists to find.

## Files

| File | Covers |
|---|---|
| `wire.py` | Transport + framing. Not a gate; imported by `gates.py`. |
| `gates.py` | Native wire: baseline generate/embed, then the v0.8.0 `tool_choice` surfaces. |
| `bridge.py` | `inferd-http` over HTTP, including tool calls on **both** stream paths. |

`wire.py` speaks each surface directly instead of going through
`inferd-client`, so the framing itself is exercised:
`[uvarint payload_len][1 byte type: 0x01 JSON / 0x02 BLOB][payload]` for
generation, NDJSON for embeddings. It picks the transport from the
platform — AF_UNIX on Unix, named pipes via `ctypes` on Windows — and
resolves socket paths the way `inferd-daemon/src/endpoint.rs` does:

- **Linux** — `$XDG_RUNTIME_DIR/inferd/`, else `~/.inferd/run/`
- **macOS** — `$TMPDIR/inferd/` (macOS rotates the per-user temp dir per
  login; the launchd plist substitutes the same path). *Not*
  `~/.inferd/run` — that fallback is Linux-only.
- **Windows** — `\\.\pipe\inferd`, `\\.\pipe\inferd-infer-embed`

Override with `INFERD_SOCK` / `INFERD_EMBED_SOCK`. Print what it
resolved, along with the active timeouts:

```sh
python3 packaging/validate/wire.py
```

## Timeouts — raise them, don't wrap the scripts

The client-side read timeouts default to 180s for a generating call and
120s for embeddings, which suits an accelerated host. They bound only how
long the *client* waits, so raising one cannot mask a daemon defect —
whereas too tight a value reads exactly like a hang:

```sh
INFERD_GEN_TIMEOUT=600 python3 packaging/validate/gates.py
INFERD_HTTP_TIMEOUT=600 python3 packaging/validate/bridge.py
```

Also `INFERD_EMBED_TIMEOUT`. Expect to need these on a **CPU-only**
target, and on macOS where Metal JIT-compiles each new kernel-shape
variant on first use — the v0.8.0 macOS leg saw one adversarial-prompt
generation exceed 180s on a memory-pressured box while `doctor` stayed
`ready` and the result was byte-for-byte correct. That leg had to build a
throwaway wrapper to widen the timeout, which is precisely the
scratch-rebuild this harness exists to stop; hence the env knobs.

A slow decode is not a failure. Confirm the daemon is healthy
(`inferdctl doctor`) before treating a timeout as a defect.

## Running

Install from the archive, wait for `inferdctl doctor` to report ready,
then:

```sh
python3 packaging/validate/gates.py            # native wire
inferd-http --listen 127.0.0.1:8080 &          # from the ARCHIVE binary
python3 packaging/validate/bridge.py           # OpenAI-compat surface
```

Both scripts assert and exit non-zero on any failure, so the exit code
is the result. Kill the bridge by PID or port when done — never with a
broad process kill.

Record alongside them, since no script checks these: `inferdctl doctor`
all-`ok`, `inferdctl status` exit 0 with every capabilities frame
relayed, the reported `accelerator` and device, and `gpu_layers` per
backend.

## Traps

Recorded because each produced convincingly wrong output rather than an
obvious error:

- **`wire_version` is 1, not 2.** The "v2" in the generation surface's
  name is the *surface* generation (ADR 0021), not the wire version.
  `inferd_proto::v2::WIRE_VERSION` has never moved. Sending 2 earns a
  correct `wire_version_unsupported` on *every* gate, which reads like a
  daemon defect.
- **Streaming frames are `{"type":"frame","block":{…}}`**, not
  `{"type":"token","text":…}`. Text arrives as incremental `delta`s.
  Reading the wrong shape prints empty text and zero tool calls while
  the terminal frames look perfect — **a green terminal frame is not a
  green gate.** Assert on content, never only on `stop_reason`.
- **`response_format` is internally tagged with `schema` at the top
  level** (`inferd-proto/src/v2/request.rs`), not the OpenAI-style
  nested `json_schema` wrapper — the bridge translates that away. The
  nested shape earns a misleading `missing field 'schema'` instead of
  the mutual-exclusion error the gate is checking for.
- **G-B is expected to hit `max_tokens`.** `required` bounds where the
  turn may *end*, not what it contains, so a model prompted against the
  constraint emits non-call text until the budget runs out.
  `tool_choice_unsatisfied: true` is the gate — raising the budget does
  not help, and the degenerate repetition is why.
- **The bridge's stream and non-stream paths are separate code.** The
  non-streaming one silently dropped `tool_calls` from the bridge's
  first release until v0.8.0, and streaming-only SDK validation missed
  it for two releases. Gate both.

`bridge.py` is stdlib-only so it runs on a bare host, but note the
trade-off: an OpenAI SDK pass previously caught two bugs (stream
default, base64) that curl-shaped checks missed. Where the host has the
SDK, run it too.
