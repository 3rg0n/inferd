# 0009. Pre-M1 open questions resolved

- Status: accepted; "no in-band version negotiation" superseded by [0021](0021-unified-v2-wire-length-prefixed-blob-framing.md) (in-band `wire_version` added in v0.4 when generation folded to one socket)
- Date: 2026-05-15

## Context

`docs/plan-v0.1.md` left three open questions for the
implementer to settle before M1 freezes the proto crate. ADR
0007 implicitly added a fourth (how does the `done` frame
expose which backend served a request). All four touch the
wire shape or the daemon's externally visible behaviour, so
they need to be settled *before* M1 begins, not during.

This ADR closes them.

## Decisions

### 1. Admin socket: ported, separate endpoint

**Question.** Port the separate admin socket pattern, or
collapse admin into the inference socket via a `subscribe:
true` request field?

**Decision.** Port the separate-socket pattern. Defaults:

- Unix: `/run/inferd/admin.sock`, mode `0600`,
  daemon-uid only.
- Windows: `\\.\pipe\inferd-admin`, ACL: current SID only.
- No TCP admin endpoint. Admin is local-only.

The admin socket carries `status` frames with `id: "admin"`
for events not tied to an inference request (e.g. the startup
`loading_model` → `ready` transition).

**Why.**
- Two sockets means admin and inference can have different
  permission posture. Admin at `0600` keeps it
  daemon-uid-only; inference at `0660` is group-shared.
- A `subscribe: true` field on the inference socket would
  mix admin events into the inference response stream,
  complicating client-side correlation.
- Two sockets are easier to threat-model independently.

### 2. Per-caller identity enforcement

**Question.** Which transports enforce kernel-attested
caller identity?

**Decision.**
- **Unix**: `SO_PEERCRED` on Linux, `LOCAL_PEERCRED` on
  macOS. Always on.
- **Windows**: `GetNamedPipeClientProcessId` →
  `OpenProcessToken` for SID. Always on.
- **Loopback TCP**: identity reduces to API-key (if
  configured) plus remote-address text for log correlation.
  No kernel attestation. Documented as a reduced-guarantee
  transport in `docs/protocol-v1.md` and
  `THREAT_MODEL.md` F-7/F-8.

Identity is recorded in the activity log on every accept
and on every `request_done`/`request_error` record.

**Why.** Defence in depth. Socket ACLs say "who *can*
connect"; peer credentials say "who actually did." For a
host-wide daemon used by multiple middlewares, knowing the
caller is cheap and stops one bad middleware from
impersonating another in logs and (v0.2+) per-caller policy.

### 3. Protocol versioning: separate-socket-per-version

**Question.** In-band version negotiation, or
separate-socket-per-version?

**Decision.** Separate-socket-per-version. v1 is on the
default endpoint; v2 will be on `/run/inferd/infer-v2.sock`
(Unix), `\\.\pipe\inferd-infer-v2` (Windows),
`127.0.0.1:47322` (TCP, opt-in). No in-band capability
exchange.

Backwards-additive changes within v1 (new optional fields
that older servers MUST ignore and older clients MUST NOT
require) are acceptable if and only if every existing v1
server already ignores unknown fields. v0.1 enforces "unknown
fields ignored on parse" in the proto crate.

**Why.** No protocol-versioning negotiation logic to test.
Migration story is "run both sockets during the transition
window" — clearer than capability negotiation. ADR 0008
already encodes this; this ADR ratifies it as a settled
question.

### 4. Backend identity in the `done` frame

**Question.** Does v1 expose which backend served a request,
and how?

**Decision.** Yes, via a `backend` field on `done` frames.
Value is the `Backend::name()` string — e.g. `llamacpp`,
`mock`, in v0.2 `anthropic`, `bedrock`, etc. Diagnostic only;
app logic must not branch on backend identity (encoded as a
caller obligation in ADR 0007).

This is a v1-additive of ADR 0008.

**Why.** ADR 0007 requires this for routing observability.
Hiding it forces operators to read the activity log for a
question every debug session asks ("which backend was that?").

## Consequences

- M1's proto crate freezes with these decisions baked in. No
  open questions remain about wire shape, socket layout, or
  identity enforcement.
- The `docs/plan-v0.1.md` "Open questions for the
  implementer" section is replaced with a pointer to this ADR.
- Each decision adds work — admin socket support across
  three platforms, peer-credential extraction across three
  platforms, dual-socket lifecycle, `Backend::name()` plumbing
  through the response stream. None are large; all are
  in-scope for M1–M4 as scheduled.

## References

- `docs/plan-v0.1.md` — open questions section, now
  superseded.
- ADR 0007 — backend identity in `done` frame requirement.
- ADR 0008 — protocol versioning rule (separate socket).
- `docs/protocol-v1.md` — wire spec reflecting these
  decisions.
- `THREAT_MODEL.md` F-7, F-8 — peer-credential and TCP
  caveat findings.
