# 0024. WSL relay: containerized middleware reaches a Windows-host daemon over a Unix socket

- Status: accepted
- Date: 2026-07-09

## Context

inferd is one host-wide warm model, many consumers ([ADR 0012](0012-one-warm-model-per-inferd-process.md)).
On Windows, the generation socket is a named pipe (`\\.\pipe\inferd`)
secured by a **protected SDDL DACL granting `GENERIC_ALL` to the
launching user's SID and nobody else** (THREAT_MODEL F-7,
`windows_security.rs`). Authorization on Windows is therefore the OS
check *"are you the same Windows user who started the daemon?"*, applied
at pipe-open — not an app-level token. The daemon binds **no inbound
network listener** ([ADR 0022](0022-no-inbound-network-listener-deprecate-loopback-tcp.md)).

A common deployment: the daemon runs on the **Windows host** (one
instance, serving native Windows apps), while **middleware runs in WSL2
and/or Podman/Docker containers inside WSL2**. We want that middleware to
consume inferd **without** running a second, duplicative daemon in WSL,
and **without** loopback TCP (which still traverses the TCP/IP stack even
on the loopback interface, unlike a kernel-buffer IPC handoff).

The obstacle is structural, not a permissions knob: **WSL2 is a separate
Linux VM with its own kernel.** A Linux process cannot open an NT-kernel
named-pipe object — there is no VFS path or syscall that targets
`\\.\pipe\...` from Linux, so "configure WSL to allow the pipe" cannot
exist. The one thing that *does* cross the VM boundary natively is a
socket/stdio channel, plus WSL's `.exe` interop (WSL can launch a Windows
binary, which runs **as the launching Windows user**).

This is why an HTTP↔IPC bridge "works" while direct pipe access does not:
the bridge is a *Windows* process (same OS as the daemon) reached over
the *network*, which WSL bridges out of the box. But HTTP is not the only
— or leanest — way to carry those bytes across.

## Decision

Ship a **first-party relay binary** (working name `inferd-wsl-relay`)
that bridges the Windows named pipe to a Unix domain socket the WSL side
(and containers within it) can dial. The daemon is **unchanged**; the
relay is a consumer-side transport shim, not a daemon feature.

```
Windows host:  inferd daemon  →  \\.\pipe\inferd     (DACL: launching user only)
                                       ▲
                                  inferd-wsl-relay.exe  (Windows proc; WSL interop
                                       │                  LAUNCHES it AS the launching
                                       │ bytes over        user — interop carries NO
                                       │ AF_VSOCK /        payload, only the launch)
                                       │ loopback, NOT stdio
WSL2 distro:   inferd-wsl-relay (linux) → $XDG_RUNTIME_DIR/inferd/inferd.sock
                                       ▲ bind-mount
Podman/Docker: middleware → inferd-client UDS path → native frames
```

- **First-party, not `npiperelay`+`socat`.** A small inferd-owned Rust
  binary avoids two external runtime deps, ships in the release, and can
  be frame-cap-aware (64 MiB, invariant #7) and log through the same
  activity surface. It is a byte-faithful stream shuttle — it does **not**
  parse or reshape frames (that would make it a second wire
  implementation to freeze; it must stay a dumb pipe).
- **Interop LAUNCHES the Windows connector; it does NOT carry the bytes.**
  A de-risk spike (2026-07-09, see "Spike findings" below) proved WSL
  `.exe` interop stdio silently **truncates output past ~512 KiB** — fine
  for interactive generation/embed frames (small), fatal for multi-MB
  image attachments. So the Windows-side connector must exchange payload
  with the Linux-side listener over a **real socket** (AF_VSOCK/hvsocket,
  or the connector dialing back a loopback endpoint the Linux side hosts)
  — interop is used only to spawn the connector as the right user. Bytes
  never ride interop stdio.
- **Trust model — no new auth needed on this path.** WSL interop launches
  the Windows-side relay as the **same Windows user** who owns the daemon,
  so it satisfies the pipe's same-user DACL legitimately. Any container
  behind the WSL-side UDS is, by construction, already inside *that
  user's* WSL session. The trust boundary is "who controls your WSL
  instance" = the launching user — no cross-user escalation is possible,
  because the relay cannot run as anyone else. peercred is only diagnostic
  on Windows; the DACL is the gate.
- **Middleware consumes the existing `inferd-client` UDS path** — no new
  wire, no new client protocol, no TCP. The relay presents the exact
  frames the daemon emits.
- **Installer behaviour.** An inferd/middleware installer that detects it
  is running inside WSL and finds the relay socket present does **not**
  install a second daemon; it wires the consumer to the UDS. (Detection +
  WSL-side setup script ship with inferd; middleware links `inferd-client`.)

## Consequences

**Easier:**

- One warm model on the Windows host serves both native Windows apps and
  WSL/containerized middleware — no duplicate model in memory (ADR 0012
  posture holds across the VM boundary).
- Consumers use native IPC (UDS), not loopback TCP — leaner per-frame,
  and consistent with ADR 0022 (no network listener anywhere).
- No app-level token/TLS on this path: OS-attested same-user identity via
  WSL interop + the pipe DACL is the whole auth story.
- Middleware code is unchanged whether the daemon is local-Linux or
  Windows-host-via-relay — both are a UDS to `inferd-client`.

**Harder / costs:**

- A new binary to own (the relay), built for both Windows (pipe side) and
  Linux (UDS side), plus a WSL-side setup step and a bind-mount recipe for
  containers.
- The relay is a single point on the path; it must handle reconnect and
  per-connection fan-out (one UDS accept ↔ one pipe instance) cleanly.
- Cross-user access is intentionally impossible — a *different* Windows
  user's WSL cannot reach the daemon. That is correct (matches the DACL),
  but means multi-user hosts need one daemon per user (already the
  per-user posture).

**Explicitly NOT done:**

- **No daemon change.** No network listener (ADR 0022 preserved), no new
  wire surface (the frozen v2/embed/admin surfaces are untouched — the
  relay carries them verbatim).
- **No frame parsing in the relay.** It is a byte shuttle; it never
  becomes a second wire implementation.
- **This does not replace the HTTP bridge** ([ADR 0020](0020-inferd-http-bridge-is-a-separate-process.md)).
  The `inferd-http` bridge remains the surface for OpenAI-compat tooling
  and for consumers that genuinely need network reach + token auth. This
  relay is the leaner path for first-party middleware that speaks inferd's
  native frames and lives in WSL beside a Windows-host daemon. ADR 0020
  Surface B (native frames over localhost TCP) stays available as the
  portable fallback where a relay isn't wanted.

## Spike findings (2026-07-09)

A throwaway spike (standalone crate, single-binary two-mode relay + a
Windows named-pipe echo server + Linux UDS pump) validated the design on
Windows 11 + WSL2 Ubuntu against the real b9850 daemon:

- **Interop stdio is byte-clean** — all 256 byte values (incl. `\n`,
  `\r`, `\0`, `0x1a`) round-tripped exactly through a WSL-launched Windows
  `.exe`. No text translation. ✓
- **Real generation round-trips end to end** — probe → UDS → interop-
  launched connector → `\\.\pipe\inferd` → daemon returned
  `answer="Paris", backend=llamacpp` with **dial=1 ms, total=778 ms**
  (overwhelmingly model time; relay/interop overhead negligible). ✓
- **Same-user-DACL trust holds** — interop ran the connector as the
  launching Windows user; it opened the same-user-only pipe with no auth,
  no error. ✓
- **HARD LIMIT: WSL `.exe` interop stdout silently truncates past
  ~512 KiB.** Reproduced with a size sweep: 300 KB / 400 KB / 450 KB /
  500 KB round-trip exact; **600 KB → truncated to ~299 KB**; 10 MB →
  ~0.5 MB. Isolated to interop's stdio transport itself (file→file echo,
  no relay/pipe/backpressure involved) — not fixable in relay code, and
  unaffected by `copy_bidirectional` vs. hand-rolled pumps.

**Consequence for the design (folded into Decision above):** interop is
for **launch only**; payload must cross the WSL↔Windows boundary over a
real socket (AF_VSOCK/hvsocket or a loopback endpoint the Linux side
hosts and the connector dials back), never over interop stdio. Small
interactive traffic (generation/embed) would have worked over stdio, but
multi-MB image attachments would silently corrupt — so the socket-carry
channel is mandatory for a correct implementation, not an optimisation.
The full `inferd-wsl-relay` build is deferred pending selection of that
carry channel (task #181).

## References

- ADR 0022 — no inbound network listener (this path uses IPC + relay, not
  TCP into the daemon).
- ADR 0020 — the HTTP bridge (complementary; different consumer class).
- ADR 0012 — one warm model per process (preserved across the VM
  boundary — the whole point of not duplicating the daemon in WSL).
- ADR 0014 — the CLI/consumers are reference middleware, not privileged
  surfaces (same posture: the relay is a consumer-side shim).
- `crates/inferd-daemon/src/windows_security.rs` — the same-user pipe
  DACL that makes the relay's trust model sound.
