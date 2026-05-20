# 0014. The inferd CLI is a reference middleware, not a privileged surface

- Status: accepted
- Date: 2026-05-20

## Context

The `crates/inferd/` binary (renamed from `inferdctl` in
v0.1.10) is the operator-facing command-line tool — `inferd
status`, `inferd watch`, `inferd pull`, `inferd doctor`, and a
planned `inferd -p "..."` prompt mode that lands later. It ships
in every release tarball alongside `inferd-daemon`.

A natural temptation, as the CLI grows, is to give it special
internal access — a non-public daemon API for operations the CLI
needs but external consumers don't, an "internal" subcommand
surface, or shortcuts that bypass the wire protocol. This is the
shape that quietly bites projects later: today's "convenient
internal hook" becomes tomorrow's "and now we have to support
that hook for everyone, or break the CLI's behaviour."

ADR 0013 commits inferd to being a gateway whose contract is the
wire protocol. That contract has to apply to *every* consumer
including our own. If the CLI gets a special path, the wire
protocol is no longer the contract — the wire protocol *plus
some private CLI affordances* is the contract, and that's a
weaker thing.

## Decision

The `inferd` CLI is a **reference middleware client**. It is a
peer of every other consumer (thlibo, IDE plugins, agents, web
apps, future external middlewares). It uses the same crates
(`inferd-client`, `inferd-daemon`'s public library surface) any
external consumer would use. It has:

- **No private daemon API.** Everything the CLI does is doable
  by any other consumer with the same crate dependencies.
- **No "internal" subcommand surface.** Subcommands are designed
  for the operator persona; the *implementation* uses public
  library APIs.
- **No special-cased socket paths or auth bypass.** The CLI
  connects to the same admin / inference sockets every other
  consumer would, with the same identity model (per-caller UID /
  SID on Unix and Windows respectively).

Concretely, the four subcommands today break down as:

| Subcommand | Library surface used |
|---|---|
| `inferd status` | `inferd-client::AdminClient` connects to admin socket, reads one frame, prints it |
| `inferd watch` | `inferd-client::AdminClient` connects to admin socket, streams forever |
| `inferd pull` | `inferd-daemon::fetch::fetch_model` + `inferd-daemon::store::ModelStore` from the public library — operates on the CAS store directly without involving a running daemon |
| `inferd doctor` | Composes `AdminClient` + `ModelStore` + `ConfigFile` to produce a punch list — exactly what an integrator's diagnostic tool would do |

The future `inferd -p "..."` will use `inferd-client::Client`'s
`generate` method exactly the way thlibo will — render tokens to
stdout instead of a UI surface, but the same library API.

We ship the CLI in the release tarball as a packaging
convenience. It's the most-likely-needed consumer for a fresh
install (operator wants to verify the daemon came up, pre-warm a
model, diagnose connection issues), so bundling it saves the
step of "now go install another tool to do anything." But that's
*convenience*, not architectural status. If a future operator
team forks the CLI, replaces every subcommand with a different
shape, and ships their own version, that fork is exactly as
capable as our in-tree CLI.

## Consequences

### Why this is the right shape

- **The wire protocol stays the contract.** No private APIs
  means no two-tier consumer model. Whatever the CLI can
  observe or do, every external consumer can also observe or
  do. ADR 0013's gateway model holds without exception.
- **Every CLI subcommand is implicitly a contract test.**
  Implementing `inferd doctor` forced us to confirm that the
  admin socket exposes the lifecycle state in a queryable form,
  that `ModelStore` can be opened independently of a running
  daemon, that `ConfigFile::load` is publicly callable. Each of
  those was an externally-useful library surface that the CLI
  validated in the act of using. That validation transfers
  directly to external consumers.
- **CLI feature requests become library feature requests.** If
  someone says "I wish `inferd doctor` would also show me the
  admission queue depth," the right response is "let's expose
  that on the admin socket so any consumer can show it,
  including doctor." We don't add a private daemon API for the
  CLI to introspect queue state.
- **Consumers can replace the CLI without losing capability.**
  A team that wants a richer / GUI / web doctor tool builds it
  using the same crates the CLI uses. They aren't second-class
  consumers.

### What this costs

- **Some operator conveniences cost extra protocol surface.** A
  feature that would have been a one-line "open the daemon's
  inner state directly" call becomes a more-considered "expose
  this state on the admin socket so any consumer can use it"
  design discussion. That's the right discussion to have, but
  it slows individual CLI features down.
- **The CLI can't do anything that requires daemon-internal
  cooperation we wouldn't want to expose externally.** For
  example: there's no `inferd inject-test-frame --backend mock`
  shortcut that the CLI gets but middleware doesn't. If we
  wanted that for testing, we'd need to expose it on the wire
  protocol behind a flag — which we won't, so the CLI doesn't
  get it.
- **`inferd pull` is a *partial* exception** in that it
  bypasses the daemon entirely and writes directly to the CAS
  store. That's not a daemon-internal API though — it's the
  same `ModelStore` library any external operator tool would
  use to populate the store ahead of bringing the daemon up.
  Documented as such in the CLI's source.

### What this explicitly does not change

- **The CLI continues to ship in the release tarball.** This
  ADR is about how the CLI is built, not whether it's
  distributed. Bundling stays.
- **The CLI is allowed to be a thin worked example.** It
  doesn't have to be the most polished consumer of the library —
  just a correct one. If thlibo's UX wraps a richer prompt-mode
  experience around `inferd-client::Client`, that's exactly
  what we hoped for.
- **ADR 0013 stays load-bearing.** This ADR is the corollary:
  if the daemon is a gateway and its contract is the wire
  protocol, then our own CLI uses the same wire protocol.

## Alternatives considered

- **Give the CLI a private daemon API for "operator
  conveniences."** Rejected. Fast-tracks short-term CLI
  features at the cost of bifurcating the consumer model.
- **Move the CLI into the daemon as a `--cli` mode of the
  daemon binary.** Rejected. Conflates two roles in one binary
  (long-running service + interactive operator tool); makes the
  daemon's surface area larger; doesn't actually save anything
  because the daemon already has to expose the admin socket for
  external consumers anyway. The current "two binaries, both
  thin" shape is cleaner.
- **Don't ship the CLI; tell operators to write their own.**
  Rejected as too austere. Most operators / first-time users
  don't have the appetite to write a `doctor` tool from
  scratch. Shipping a reference one is a small cost for a real
  UX win.

## References

- ADR 0006 — lean-core (this ADR is consistent: the CLI is
  consumer-facing, but it's *outside* the daemon, so lean-core
  is unaffected by what the CLI does).
- ADR 0013 — the gateway framing this ADR is the corollary to.
- ADR 0015 — v2 wire protocol; will define the surface the CLI
  consumes for `-p` prompt mode and any future v2-aware
  subcommands.
- `crates/inferd/src/main.rs` — the implementation that this
  ADR's invariants apply to.
