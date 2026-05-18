# 0010. Narrow HTTPS exception for first-boot model bootstrap

- Status: accepted
- Date: 2026-05-18

## Context

[ADR 0006](0006-lean-core-ecosystem-extensions.md) established
the lean-core posture: HTTP, OpenAI-compat, web UI, and
similar surfaces are not part of `inferd-daemon`. They live as
separate processes that consume inferd over IPC.

Pre-GA work has surfaced a real first-boot problem. The Gemma 4
E4B GGUF that v0.1 ships against is ~5 GB. GitHub Actions
release assets cap at 2 GB per file (and even that's
gracelessly slow for users). Bundling the model into the
release tarball is not an option.

The obvious lean-core answer would be a separate `inferd-fetch`
binary the daemon shells out to. We considered it. The
operational reality:

- The fetch surface is **one HTTPS GET** against **one
  configured URL** with **one expected SHA-256**. Not arbitrary
  registry browsing, not OAuth flows, not OCI manifest parsing.
  The attack surface is genuinely tiny.
- A separate binary doubles packaging burden: two cosign
  signatures, two SBOM entries, a second `Command::spawn`
  exception in the daemon's "no subprocess engines" rule.
- Operators see one product with one systemd unit. Splitting
  fetch into a sibling binary leaks an implementation detail
  into the install story for no security gain.
- The pattern matches Stable Diffusion WebUI, CUDA Toolkit's
  network installer, Steam's bootstrap installer: small launch
  binary, large content fetched on first run, single process
  owns its dependencies.

ADR 0006's lean-core principle is about not bundling **product
features** (HTTP API, web UI, OpenAI-compat). Bootstrapping a
vendor-pinned binary blob is not a product feature; it is
installation plumbing forced by the GitHub release-asset size
constraint.

## Decision

The daemon may issue **outbound HTTPS** for a single, narrowly
scoped purpose: first-boot bootstrap of a pinned GGUF model
named in `~/.inferd/config.json`.

Concretely, the daemon may:
- Make one `HTTPS GET` to `model.source_url` from the config.
- Stream the response body to disk.
- Verify the result against `model.sha256` with constant-time
  compare (THREAT_MODEL F-5).
- Quarantine on mismatch (F-6); atomic rename on success.

The daemon **may NOT**:
- Implement an inbound HTTP server.
- Implement an OpenAI-compatible REST surface.
- Implement a web UI, dashboard, status page, or any other
  HTTP-served surface.
- Browse, search, or enumerate models from any registry.
- Make HTTP requests for any resource other than the
  configured model URL.
- Make HTTP requests after the daemon has reached the `ready`
  state. (Once `ready`, the daemon does no network I/O for any
  reason in v0.1.)
- Make HTTP requests during a `restarting` reload. (Reloads
  happen against models already on disk; if a new model needs
  to be fetched, the operator restarts the daemon process.)

Implementation lives in `crates/inferd-daemon/src/fetch.rs` —
one module, one entry point (`fetch_model`), zero re-export of
HTTP types from the daemon's public API.

## Consequences

**Why this is the right shape:**

- One binary. One systemd unit. One signed artefact. The
  packaging story stays clean.
- The fetch attack surface is genuinely small. One pinned URL,
  one pinned SHA, one file on disk. We are not Ollama; we are
  not LM Studio. We do not parse arbitrary upstream registries.
- The narrow exception is testable: any future PR adding HTTP
  for a non-model resource fails review against this ADR.
- Refactoring fetch out of the daemon later is mechanical —
  the module is self-contained, has no shared state with the
  inference path, and could become a sibling binary in v0.2 if
  GitHub raises the release-asset cap (unlikely) or we move
  to a different distribution channel that bundles the model.

**What this costs:**

- The "no HTTP in the daemon" rule from ADR 0006 needs a
  scope-narrowing footnote. ADR 0006 stays accepted; this ADR
  carves the named exception.
- A new transitive dep tree: `ureq` + `rustls`. Audited via
  `cargo audit` on every CI run; no new crates with weak
  maintenance signals.
- A small surface for misconfigured operators to expose the
  daemon to network problems at boot. Documented: `auto_pull`
  is configurable in `~/.inferd/config.json`; operators in
  air-gapped environments set `auto_pull: false` and place
  the model at `models_dir/<filename>` themselves.

**What this explicitly does not change:**

- The wire protocol (`docs/protocol-v1.md`) is untouched. No
  new fields, no new frames, no breaking changes.
- The inference perimeter is untouched. No HTTP from clients;
  NDJSON-over-IPC remains the only inference transport.
- ADR 0005 (consume libllama via FFI, not subprocess) is
  untouched. The daemon still has zero subprocess engines.
- ADR 0007 (operator-policy routing) is untouched. v0.1 still
  has a no-op router; cloud routing is v0.2.

## Alternatives considered

- **Separate `inferd-fetch` binary** the daemon shells out to.
  Rejected for the reasons in §Context: doubled packaging
  surface, additional `Command::spawn` exception, no security
  gain given the tiny fetch surface. May be revisited if the
  fetch surface grows materially (e.g. multi-model support,
  registry browsing).
- **Bundle the model in the release tarball.** Rejected: GitHub
  Actions release-asset cap is 2 GB; the model is ~5 GB. Even
  if the cap were raised, downloads-per-release would dominate
  GitHub bandwidth quotas.
- **Operator-driven manual fetch.** Rejected as the *only*
  option but kept as a *fallback*: setting `auto_pull: false`
  in the config disables in-daemon fetch. Operators in
  locked-down environments use `wget`/`curl` to populate
  `models_dir/<filename>` themselves; the daemon verifies the
  SHA on startup and refuses to load a mismatch.

## References

- ADR 0005 — libllama via FFI; daemon consumes one C library
  but spawns no subprocess engines.
- ADR 0006 — lean-core posture this ADR carves an exception
  to.
- ADR 0008 — protocol v1 frozen; unchanged by this ADR.
- `crates/inferd-daemon/src/fetch.rs` — implementation site.
- `crates/inferd-daemon/src/config_file.rs` — `auto_pull`
  configuration knob.
- `THREAT_MODEL.md` F-5 / F-6 — covering the verify and
  TOCTOU paths the fetch module relies on.
