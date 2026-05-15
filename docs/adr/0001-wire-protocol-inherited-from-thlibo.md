# 0001. Wire protocol v1 inherited byte-for-byte from thlibo

- Status: superseded by [0008](0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md)
- Date: 2026-05-14

## Context

thlibo v0.1 already ships a working NDJSON-over-IPC protocol that is
in production use with Claude Code's PreToolUse hook, tested with
measured compression ratios, and carries the v0.1 test suite. inferd's
immediate reason to exist is to be a drop-in replacement for thlibo's
embedded daemon.

We could:

1. Invent a fresh protocol that expresses inferd's ambitions (backend
   adapters, KV-cache multiplexing, auth) cleanly from day one.
2. Match thlibo's v1 exactly and only diverge in later versions once
   real-world need has exposed what's missing.

## Decision

Match thlibo v1 exactly. Byte-for-byte. Frame cap, field names, role
enum, grammar field, response types — all identical. When inferd
needs to extend the shape, it introduces v2 on a separate socket
path.

## Consequences

**Why this works:**

- thlibo v0.2 migration is an import swap, not a marshalling rewrite.
  `internal/daemon/` and `internal/ipc/` delete cleanly; middleware
  tests that exercised the protocol keep passing.
- The protocol is already battle-hardened: cancellation on
  disconnect, frame-size caps, GBNF pass-through, image-token-budget
  validation. Re-deriving these from scratch would just re-discover
  the same edge cases.
- A clear versioning story: v2 is a separate socket. No in-band
  negotiation, no capability exchange. Run both during migration.

**Cost:**

- v1 carries thlibo-shaped assumptions (single-turn, single model,
  single active generation). Multi-model routing, KV sharing, and
  auth-header handshakes will need v2.
- thlibo's protocol document was never written up formally; we had to
  write `docs/protocol-v1.md` by reading the Go source. One-time cost,
  but it's borne now.

## References

- thlibo `internal/ipc/protocol.go` — authoritative source of the
  frame shape at the time of this decision.
- thlibo `.plan/thlibo-spec.md` §"IPC" and §"Protocol".
- thlibo `THREAT_MODEL.md` — finding #5 (frame cap) is carried
  over verbatim.
