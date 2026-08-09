# Consuming inferd across a VM / container boundary

inferd's contract ends at a **local IPC endpoint** speaking the wire
protocol:

| Surface | Unix | Windows |
|---|---|---|
| generation (v2) | `inferd.sock` | `\\.\pipe\inferd` |
| embeddings | `infer.embed.sock` | `\\.\pipe\inferd-infer-embed` |
| admin | `admin.sock` | `\\.\pipe\inferd-admin` |

Authorization is kernel-attested peer identity — Unix `SO_PEERCRED` +
`inferd-users` group (socket `0660`); Windows same-user pipe DACL. The
daemon binds **no network listener** ([ADR 0022](adr/0022-no-inbound-network-listener-deprecate-loopback-tcp.md)),
and bridges live as separate consumer processes ([ADR 0006](adr/0006-lean-core-ecosystem-extensions.md)/[0014](adr/0014-inferd-cli-is-a-reference-middleware.md)).

When your middleware runs in the **same OS/kernel** as the daemon, you
just dial the endpoint with `inferd-client` (or a hand-written client)
and you're done. This guide is about the harder case: your middleware
runs in a **different memory domain** than the daemon — most commonly
**middleware in WSL2 / Docker / Podman, daemon on the Windows host** (or
vice-versa). Crossing that boundary is *your* choice of mechanism;
inferd deliberately does not bless one ([ADR 0024](adr/0024-wsl-relay-for-containerized-middleware.md)).
Below are the options we validated, with tradeoffs — including the ones
that don't work, so you don't rediscover them.

**Why the consumer owns the bridge (app-mapping fidelity).** Beyond the
transport limits below, there's a positive reason inferd doesn't ship one
shared relay: it would funnel every consumer through a single bridge
process, so the daemon — and its peer-credential auth + activity log —
would see **one identity** for all of them, erasing *which app made which
request*. When each middleware owns its bridge, each keeps an isolated
channel and its own identity end-to-end, preserving per-app mapping. A
bridge you build protects that isolation; a shared one we built would
destroy it.

> **TL;DR recommendation:** if you can, **run an inferd daemon in the same
> Linux domain as your middleware** (in WSL2, or the container host) and
> use a native UDS. It's the only option with zero transport compromise.
> Everything else that crosses the WSL2↔Windows boundary trades away
> either "no TCP" or "no elevation" or "no fragility".

---

## Option A — Co-locate the daemon (recommended, zero compromise)

Run inferd **inside the same Linux memory domain** as the middleware:

```
WSL2 distro (or container host):
  inferd daemon  →  $XDG_RUNTIME_DIR/inferd/inferd.sock   (UDS 0660)
                          ▲ bind-mount
  Podman/Docker:  middleware → inferd-client dial_uds → native frames
```

- **Transport:** native Unix domain socket. No bridge, no networking, no
  interop, no cross-VM hop.
- **Auth:** real `SO_PEERCRED` — the daemon sees the middleware's actual
  uid/gid (works when uids align; for containers, run as a uid in
  `inferd-users` or bind-mount accordingly).
- **Containers:** bind-mount the socket in:
  `-v $XDG_RUNTIME_DIR/inferd/inferd.sock:/run/inferd/inferd.sock`
  then point the client at the mounted path.
- **Cost:** if you *also* need native Windows apps served, that's a
  *second* daemon on the Windows host. Per [ADR 0012](adr/0012-one-warm-model-per-inferd-process.md)
  as read in [ADR 0024](adr/0024-wsl-relay-for-containerized-middleware.md),
  one warm model **per memory domain** is the intended model — the VM
  boundary is a real isolation/memory boundary, not wasteful duplication.

This is the path inferd recommends and has validated end-to-end on WSL2
(see `docs/v0.4-validation.md` / `docs/v0.5-validation.md`).

---

## Option B — `inferd-http` bridge (network + OpenAI-compat + token auth)

Run the separate `inferd-http` bridge process ([ADR 0020](adr/0020-inferd-http-bridge-is-a-separate-process.md))
next to the daemon. It consumes the daemon over IPC and exposes
**OpenAI-compat HTTP** — `/v1/chat/completions` (stream + non-stream),
`/v1/embeddings`, `/v1/models`, `/health` — for OpenAI-SDK tooling.

ADR 0020 also sketched a **Surface B**: inferd's *native* frames over a
localhost port, so first-party middleware could write one schema and dial
a port instead of branching on pipe-versus-UDS. **That surface was never
built and is not planned.** ADR 0024 removed its motivation — a consumer
crossing a VM boundary owns the bridging, because one shared relay would
collapse every consumer into a single peer identity at the daemon. If you
want native frames across a boundary, that is Option C below, and the
relay is yours. Nothing here serves native frames over TCP.

Cross-VM consumers reach the bridge's port; on WSL2 with mirrored
networking (or default localhost forwarding) a `127.0.0.1:PORT` the
bridge binds on the Windows host is reachable from inside WSL2 as
`localhost:PORT`, forwarded over hvsocket **by WSL** (supported,
MS-maintained).

- **Transport:** loopback TCP (WSL forwards it over hvsocket internally).
- **Auth:** the bearer token terminates **at the bridge**, not the daemon
  — peer-credential identity does not survive a network hop. A
  non-loopback bind refuses to start without `--token`. **TLS is not in
  the bridge**: it speaks plain HTTP and expects a reverse proxy in front
  if you need transport encryption.
- **Cost:** it's TCP under the hood (validated working; leaner than a
  remote network call but not a kernel-buffer IPC handoff). Choose this
  if you want one host daemon serving cross-VM consumers and accept TCP.

---

## Option C — Roll your own relay (if you accept the tradeoffs)

If you want to bridge the boundary yourself, the validated mechanism is
**WSL localhost-forwarding**: bind a loopback listener on the daemon's
side, reach it as `localhost:PORT` from the guest. This is what Docker
Desktop's Windows backend effectively does. Same tradeoff as Option B
(TCP underneath, terminate your own auth at the relay). A relay is just
your process bridging that loopback port to the middleware's UDS — inferd
provides no binary for it, because the mechanism is a few lines and the
auth/tradeoff choices are yours.

---

## What does NOT work (validated dead ends — don't repeat these)

We spiked these thoroughly on Windows 11 24H2 + WSL 2.9.3
(see [ADR 0024](adr/0024-wsl-relay-for-containerized-middleware.md)
"Spike evidence"):

- **❌ Relaying the pipe over WSL `.exe`-interop stdio** (launch a Windows
  connector from WSL, pipe bytes through its stdin/stdout — the
  npiperelay pattern). **Interop stdio truncates bulk transfers past
  ~512 KiB.** Fine for small/interactive frames (a real generation
  round-trips in <1 s), but a multi-MB **image attachment silently
  corrupts**. Reproduced with fd-redirection, with `socat EXEC:`, and
  with correct drain-before-exit discipline — the cap is in interop
  itself, not fixable in relay code. (npiperelay works for Docker/MySQL
  because those never push >512 KiB one-shot.)

- **❌ Raw Hyper-V sockets (AF_HYPERV ↔ AF_VSOCK) from a third party.**
  The API is real and documented, and WSL uses it internally — but on
  WSL2 it's privilege-walled for third parties. Host→guest needs the WSL
  VM's `VmId`, which requires **Hyper-V Administrator** (`hcsdiag` refuses
  otherwise; the WSL VM is hidden from HCS). Guest→host to a third-party
  host listener does not connect even after registering a
  `GuestCommunicationServices` service GUID (admin), opening the WSL
  Hyper-V firewall, and a fresh `wsl --shutdown`. Not worth building on —
  it's WSL-private plumbing.

- **❌ A Linux process opening `\\.\pipe\inferd` directly.** There is no
  VFS path or syscall from a Linux VM into the Windows NT object
  namespace. "Configure WSL to allow the pipe" is not a thing.

---

## Choosing

| You want… | Use |
|---|---|
| Cleanest, no compromise, and can co-locate | **A — daemon in the Linux domain (UDS)** |
| One host daemon + OpenAI-compat reach, accept TCP | **B — `inferd-http` bridge** |
| One host daemon + **native** frames across the boundary | **C** — no first-party option exists |
| One host daemon, custom relay, accept TCP + own your auth | **C — localhost-forwarding relay** |
| No TCP **and** cross-VM **and** supported **and** bulk-safe | Not achievable on WSL2 — pick A |

The client libraries (`clients/go`, `clients/py`, `clients/ts`, and the
Rust `inferd-client`) speak the wire on whatever UDS/pipe/loopback
endpoint you point them at — so your code is the same regardless of which
option carries the bytes.
