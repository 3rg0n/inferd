# 0004. MIT license, not Apache-2.0

- Status: accepted
- Date: 2026-05-14

## Context

inferd is infrastructure intended to be vendored by third-party
middlewares, including commercial ones. The two serious candidates
are MIT and Apache-2.0.

Apache-2.0 is the gold standard for vendorable infrastructure (used
by Kubernetes, OpenTelemetry, containerd) because it carries an
explicit patent grant. MIT is shorter, simpler, and matches the
owning organisation's existing thlibo project.

## Decision

MIT.

## Consequences

**Why MIT:**

- Matches thlibo's own license, so a thlibo-to-inferd migration has
  no license-compat concern and dual-licensed contributions aren't
  needed.
- Shorter boilerplate on contribution acceptance.
- The owner has already chosen MIT for the adjacent thlibo project.

**Cost:**

- No explicit patent grant. For the scope of inferd (model-loading
  plumbing, process management, NDJSON framing) this is extremely
  unlikely to matter — the underlying art is decades old — but it's
  worth naming the trade-off.
- Some enterprise legal teams default-reject MIT dependencies.
  Practically, the ones that matter (Anthropic, OpenAI, AWS all
  accept MIT) are fine. If a specific adoption story later demands
  Apache-2.0, we can relicense (all contributors will be asked to
  agree before any relicense happens).

## References

- [choosealicense.com MIT vs Apache](https://choosealicense.com/appendix/#mit)
- thlibo LICENSE (MIT).
