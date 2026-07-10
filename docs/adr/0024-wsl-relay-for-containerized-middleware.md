# 0024. Crossing a VM/container boundary to reach inferd is the consumer's concern

- Status: accepted
- Date: 2026-07-10

## Context

inferd is one host-wide warm model, many consumers over IPC ([ADR 0012](0012-one-warm-model-per-inferd-process.md)).
Its only contract is the wire protocol on a local endpoint — UDS on
Unix, named pipe on Windows ([ADR 0021](0021-unified-v2-wire-length-prefixed-blob-framing.md)/[0017](0017-embeddings-on-a-third-socket.md)),
authorized by kernel-attested peer identity (Unix `SO_PEERCRED` + group,
Windows same-user pipe DACL). It binds **no inbound network listener**
([ADR 0022](0022-no-inbound-network-listener-deprecate-loopback-tcp.md)),
and consumer-facing surfaces (HTTP, OpenAI-compat, bridges) live as
**separate processes**, never in the daemon ([ADR 0006](0006-lean-core-ecosystem-extensions.md), [ADR 0014](0014-inferd-cli-is-a-reference-middleware.md)).

A common deployment raised the question this ADR answers: the daemon on
a **Windows host**, middleware in **WSL2 / Docker / Podman inside WSL2**.
Middleware there cannot open the daemon's Windows named pipe directly (a
Linux process has no handle into the NT object namespace), so *something*
must bridge the WSL2↔Windows VM boundary. An earlier draft of this ADR
proposed inferd ship a first-party `inferd-wsl-relay` binary to do it.

A thorough spike (2026-07-10, recorded below) tried to find a
**supported, non-TCP, cross-VM** transport worth blessing as first-party.
There isn't one. Every option is compromised in a different way, and the
"right" bridge depends on the consumer's constraints (do they tolerate
loopback TCP? can they run an elevated install step? do they push
multi-MB payloads?). Blessing one bridge would force one set of
tradeoffs on every consumer.

## Decision

**inferd does not ship a cross-boundary bridge.** The daemon's contract
ends at its local IPC endpoint + the wire protocol. Reaching that
endpoint from another VM/container/memory-domain is the **consumer's
concern**, consistent with ADR 0006/0014 (transports are separate
processes; consumers speak the wire).

**The crux — app-mapping fidelity.** This is not merely "we couldn't find
a clean transport." A single first-party relay would funnel every
consumer's traffic through **one bridge process**, so the daemon (and its
peer-credential authorization + activity log) would see **one identity**
for all of them — collapsing the mapping of *which application made which
request*. If instead **each middleware builds its own bridge**, each
consumer keeps an **isolated channel and its own identity** end-to-end,
and app-mapping fidelity is **preserved**. So consumer-owned bridging is
the design that protects per-app isolation; a shared inferd-built relay
would actively destroy it. That, more than the transport limitations
below, is why bridging belongs with the consumer.

Two things ship instead of a relay:

1. **A validated-options guide** — `docs/consuming-across-a-boundary.md`
   — documenting the transports we tested, what works, what doesn't, and
   the tradeoffs, so a consumer can implement the bridge that fits *their*
   constraints without repeating this spike.
2. **The recommended clean topology** — for WSL/container middleware, run
   an inferd daemon **in the same Linux memory domain** (in WSL2, or the
   container's host) and consume it over a native UDS via `inferd-client`.
   No bridge, no TCP, no cross-VM hop, real peercred. This reframes
   ADR 0012's "one warm model per machine" as **one warm model per
   memory/isolation domain** — the VM boundary is a real boundary; a
   daemon per side respects it rather than tunnelling through it.

The `inferd-http` bridge ([ADR 0020](0020-inferd-http-bridge-is-a-separate-process.md))
remains the answer for consumers that want network reach + OpenAI-compat
+ token auth; its Surface B (native frames over localhost) is exactly the
loopback-forwarding path a cross-VM consumer would use if they accept TCP.

## Spike evidence (2026-07-10, Windows 11 24H2, WSL 2.9.3, kernel 6.18, mirrored networking)

Why no supported+non-TCP+cross-VM transport exists:

- **WSL `.exe` interop stdio caps ~512 KiB for bulk transfers.** Native
  Windows echo moves 10 MB exact; the same bytes through interop
  (WSL→`.exe`→WSL) truncate to ~0.3–0.7 MB. Reproduced with fd
  redirection, with npiperelay's exact `socat … EXEC:` pattern, and with
  full drain-before-exit discipline — all cap. Fine for small/interactive
  frames (a real generation round-tripped in 778 ms), fatal for multi-MB
  image attachments. This is why launching a connector and piping over
  its stdio is not viable for inferd's full workload.
- **Raw Hyper-V sockets (AF_HYPERV↔AF_VSOCK) host↔guest are real and
  documented, but privilege-walled for third parties on WSL2.**
  Guest→host to `CID_HOST` routes (timeout, not unreachable — correct
  target), but a non-privileged third-party host listener is not wired
  into the WSL utility VM's connection routing; registering a
  `GuestCommunicationServices` GUID (admin) + opening the WSL Hyper-V
  firewall + a fresh `wsl --shutdown` boot still did not connect.
  Host→guest needs the guest VmId, which requires **Hyper-V
  Administrator** (`hcsdiag` refuses without it; the WSL VM is hidden
  from HCS enumeration). Docker crosses this only by running **inside**
  WSL's trust boundary (handed the VmId, private control protocol) or by
  shipping its own Windows-host backend.
- **WSL localhost-forwarding (mirrored networking) works and is
  supported** — but it is loopback TCP underneath. Legitimate, just not
  "no TCP".

Net: **{supported, no-TCP, cross-VM} is unachievable on WSL2 for bulk
payloads.** So there is nothing worth hard-coding as first-party; the
consumer picks the tradeoff.

## Consequences

**Easier:**
- The daemon stays lean and unchanged — no new binary, no cross-VM
  transport to own and maintain across WSL updates (which the spike shows
  would be fragile).
- Consumers pick the bridge that fits them: co-locate a daemon (cleanest),
  or use the `inferd-http` bridge, or roll a loopback-forwarding relay if
  they accept TCP. The guide documents each with its tradeoffs.
- The recommended path (daemon-per-memory-domain) has **zero** transport
  compromise and is already supported today.

**Harder / costs:**
- A consumer that specifically wants *one* Windows-host daemon serving
  *WSL* middleware must accept a compromise (loopback TCP via forwarding,
  or an elevated hvsocket install) — inferd won't hide that cost behind a
  blessed relay. The guide makes the tradeoff explicit rather than
  pretending one clean option exists.

**Supersedes:** the earlier draft of this same ADR (unpublished) that
proposed a first-party `inferd-wsl-relay`. That relay is **not** built —
the spike showed no transport worth blessing.

## References

- `docs/consuming-across-a-boundary.md` — the validated-options guide.
- ADR 0006 / 0014 — transports are separate processes; consumers speak
  the wire. This ADR is a direct application of that posture.
- ADR 0020 — the HTTP bridge (the answer for network + token-auth
  consumers; Surface B is the loopback-forwarding path).
- ADR 0012 — one warm model per process, now read as *per memory domain*.
- ADR 0022 — no inbound network listener in the daemon (unchanged).
