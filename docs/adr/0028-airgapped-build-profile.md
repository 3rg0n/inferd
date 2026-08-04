# 0028. The airgapped build is a default-on feature turned *off*, not an opt-in feature turned on

- Status: accepted
- Date: 2026-08-04

## Context

ADR 0010 carved a deliberately narrow exception to inferd's
no-network posture: the daemon may issue outbound HTTPS for exactly
one purpose — fetch one pinned URL, verify one SHA-256, write one
file — and never again after `ready`. That exception is what makes
install=work possible on a fresh machine.

In an airgapped or high-assurance deployment the exception is not a
convenience, it is a finding. Reviewers there do not audit *whether*
the code path is taken; they audit whether the binary **can** take
it. "It only fetches when `source_url` is set" is a runtime argument,
and runtime arguments do not survive that review — the TLS stack is
linked, the socket call is reachable, and a config file is the only
thing standing between the process and an egress attempt. Issue #145
asks for a build that cannot make the argument necessary: no HTTPS
client, no cloud adapters, provable by inspection rather than by
reading control flow.

The mechanism matters more than usual here, because the obvious
mechanism does not work.

**Cargo features are purely additive.** A feature can add a
dependency; it can never remove one. `crates/inferd-daemon/Cargo.toml`
lists `ureq` as a plain, non-optional dependency, so an added
`airgapped` feature — which is how #145 was written — would compile
out the *call sites* while leaving `ureq`, `rustls`, `ring`, and the
native-certs stack fully linked in the binary. That satisfies nobody:
the reviewer's question is about the linked artifact, and the answer
would still be "yes, the TLS stack is present." Worse, it would look
like it worked. `cargo tree` would show the deps, but only if someone
thought to check.

Two further constraints shaped the decision.

- **The two artifacts must be one code path.** A separate airgapped
  binary maintained as a fork, or an artifact built from a different
  source tree, decays. Whatever ships must be the same crates built
  with a different flag, and CI must build both on every tag so a
  break in the airgapped configuration is a red build rather than a
  discovery at the customer.
- **Airgapped must still be able to load a model.** Removing fetch
  removes the *only* way bytes get into the CAS store. An airgapped
  daemon that cannot resolve a model is not a hardened build, it is a
  broken one — and per the repo's own release bar, install=work is
  not satisfied by "then hand-edit config and hope the paths line
  up."

## Decision

### Invert the polarity: a default-on `model-fetch` feature

```toml
[features]
default = ["model-fetch"]

# Outbound HTTPS model bootstrap (ADR 0010). Default-ON. Building with
# --no-default-features yields the airgapped artifact (ADR 0028): the
# ureq/rustls/native-certs tree is not linked, and no code path in the
# binary can open a socket.
model-fetch = ["dep:ureq"]

[dependencies]
ureq = { version = "2", optional = true, default-features = false, features = ["tls", "native-certs"] }
```

The airgapped artifact is built with `--no-default-features`, plus
whatever engine features that platform needs. Because subtraction is
the only direction Cargo supports, this is not a stylistic preference
— it is the sole shape in which the guarantee is real.

The guarantee is then **mechanically checkable**, which is the whole
point:

```sh
cargo tree -p inferd-daemon --no-default-features --features dl-backends \
  | grep -E '\b(ureq|rustls|native-tls|ring|reqwest|hyper|openssl)\b' \
  && exit 1 || exit 0
```

A CI job runs exactly that and fails the build if any HTTPS crate
appears in the tree. This is the deliverable — not the `#[cfg]`
attributes, which are only how the code compiles once the dependency
is gone. Anyone can re-run it against a tag and get the same answer
without reading a line of inferd's source.

### `default = ["model-fetch"]` keeps the normal artifact unchanged

Existing build commands, install scripts, and CI legs produce a
byte-for-byte-equivalent daemon; no operator has to learn a new flag
to get what they have today. The cost of the inversion falls entirely
on the airgapped leg, which is new. This is also the reason the
feature is named for what it *adds* (`model-fetch`) rather than what
it removes: a feature named `airgapped` that had to be *on* by
default to get networking would read backwards in every build command
in the repo.

### What the feature gates

Three sites, all inside `crates/inferd-daemon`:

1. **`download_with_progress` in `fetch.rs`** — the only `ureq` call
   site in the workspace (`fetch.rs:332`). Gated out entirely.
2. **The download half of `fetch_model`** — phases 2–5 (download,
   verify-downloaded-bytes, atomic rename, manifest write). Without
   `model-fetch`, reaching the point where a download would start
   returns a new `FetchError::FetchDisabled { name }` instead.
3. **`FetchError::Transport` / `HttpStatus`** — HTTP-only variants,
   gated so the airgapped build's error enum has no unreachable
   arms.

**`fetch_model` itself is not gated, and neither is the module.** This
is the load-bearing detail. `fetch_model` is misnamed for what it
mostly does: it resolves `manifests/<name>.json` → CAS blob path,
takes the per-name writer lock, re-hashes the blob with a
constant-time compare, and quarantines on mismatch. Only the tail is
a download. An airgapped build needs every part of the head —
including the SHA verification, which is precisely the check a
high-assurance deployment cares most about. Gating the module would
delete the local-resolution path along with the network one and force
a parallel implementation, which is how the two artifacts start to
diverge.

So the airgapped daemon resolves models exactly as today, from the
same CAS store, with the same verification, and errors with
`fetch_disabled` where the networked build would have dialled out.

### `inferdctl import` — the way bytes get in

New subcommand, present in **both** artifacts:

```sh
inferdctl import --name gemma-4-e4b path/to/gemma-4-e4b-Q4_K_M.gguf
```

It SHA-256s the file, writes it into
`blobs/sha256/<aa>/<full-hash>/data` via the same partial-then-rename
producer flow `fetch.rs` uses, and writes `manifests/<name>.json`.
Optional `--expect-sha256 <hex>` verifies against an out-of-band
digest with a constant-time compare and refuses the import on
mismatch — an airgapped operator carrying a file in on removable
media has a digest from the vendor and no way to check it today.

Shipping it in both artifacts is deliberate: a subcommand that exists
only in the airgapped build is a subcommand nobody tests, and
importing a hand-downloaded GGUF is useful on a networked machine too
(it is what "operators wanting fancier transports `wget` the file
themselves" in `fetch.rs`'s own module docs has always implied,
without ever providing the tool to finish the job).

`import` is a CAS-store writer, so it is `inferdctl`'s concern, not
the daemon's — consistent with ADR 0014/0018 (the CLI is a reference
middleware over the same libraries).

### The cloud adapters need no work

`inferd-engine`'s `openai` and `bedrock` features are already
correctly `optional = true` behind `dep:reqwest` &c., and already off
by default. The airgapped leg simply does not enable them, and the
`cargo tree` assertion above covers `reqwest`/`hyper` as a regression
guard in case that ever changes. #145 listed them as work; inspection
says they are already done.

### Two artifacts per platform, one build matrix

`release.yml` gains a second build+pack pass per target:

| Artifact | Build |
|---|---|
| `inferd-<ver>-<target>.<ext>` | as today |
| `inferd-airgapped-<ver>-<target>.<ext>` | `--no-default-features --features <accel>` |

Five platforms × two artifacts = ten archives; the existing
asset-completeness check (which counts archives) is updated to match,
and both get SHA256 sidecars and cosign signatures like any other
asset. The `cargo tree` assertion runs as a gate in the same
workflow, so an airgapped archive cannot be published alongside a
tree containing a TLS stack.

**Two artifacts, not four.** Rerank (ADR 0027) is a *runtime* config
flag rather than a build feature precisely so it does not multiply
this matrix — it adds no dependency, so a build flag would buy
nothing and cost a rebuild for a config change. Build features here
are reserved for removing dependency trees and egress paths, which is
the one thing runtime configuration cannot do.

### Scope

In:

- `model-fetch` feature, `ureq` made optional.
- `#[cfg]` on the three sites above; `FetchError::FetchDisabled`.
- `inferdctl import` with optional `--expect-sha256`.
- `release.yml` second pass + updated asset count.
- CI: a `no-network-deps` job running the `cargo tree` assertion, plus
  the airgapped configuration added to the clippy/test feature matrix
  so it cannot rot the way an unbuilt configuration does.
- Docs: `README.md` install section, `docs/RELEASING.md` asset list,
  a `docs/airgapped.md` covering import → config → run.

Out:

- **A distinct crate or binary name.** Same `inferd-daemon` binary,
  same version. The artifact filename carries the distinction; the
  binary reports it via `--version` output and a boot log line so an
  operator who has lost track of which archive they installed can ask
  the process.
- **Removing ADR 0010.** The exception stands for the default build;
  this ADR adds the ability to *decline* it, and changes nothing about
  what the networked build may do.
- **An `airgapped` runtime flag.** `INFERD_NO_FETCH=1`-style
  belt-and-braces would be a runtime assertion on top of a build-time
  guarantee — the weaker claim layered over the stronger one, and one
  more thing to keep consistent. The build is the boundary.
- **Vendoring model weights into the artifact.** Licence-hostile and
  would add gigabytes; `import` is the answer.

## Consequences

**Why this is the right shape:**

- **The guarantee is provable by a third party.** `cargo tree` on the
  published configuration, no source reading, no trust in our
  `#[cfg]` discipline.
- **One code path, both artifacts.** CI builds both on every tag, so
  the airgapped configuration cannot silently stop compiling — the
  failure mode that made Tier 3 rot for five weeks.
- **The default build is untouched.** No existing command changes.
- **Airgapped is actually usable.** `import` closes the loop that
  removing fetch would otherwise open, and it lands in both artifacts
  so it is exercised by ordinary use.
- **SHA verification survives.** The half of `fetch.rs` a hardened
  deployment cares about is the half that stays.

**What this costs:**

- Ten release archives instead of five. Longer release runs, a larger
  release page, and an operator-facing "which one do I want?"
  question the README must answer in a sentence.
- `#[cfg(feature = "model-fetch")]` is a new conditional-compilation
  axis in `fetch.rs`. Contained to one module, but real.
- One more configuration in the clippy/test matrix — non-optional,
  since an unbuilt feature combination is a broken one waiting to be
  found.
- `--no-default-features` is a sharp edge: someone building the daemon
  by hand to strip a *different* default would silently lose fetch.
  Mitigated by the boot log line naming the build, and by `default`
  containing only this one feature today.

**What this explicitly does not change:**

- ADR 0010 — the HTTPS exception, unchanged for the default build.
- ADR 0011 — CAS store layout; `import` writes the same shape
  `fetch.rs` does.
- ADR 0006 — no HTTP server, in either artifact.
- Any wire surface. This is a build-configuration decision; no frame
  changes, `wire_version` unmoved.

## Alternatives considered

- **An additive `airgapped` feature (as #145 specified).** Rejected —
  not merely inferior but non-functional. Cargo features cannot
  remove a dependency, so the TLS stack would remain linked while the
  feature name claimed otherwise. The most dangerous kind of wrong:
  it would appear to work.
- **A separate `inferd-daemon-airgapped` crate.** Rejected. Either it
  duplicates code (drift) or it is a thin wrapper that still depends
  on a daemon crate carrying `ureq` (no guarantee). The dependency has
  to be optional at the crate that declares it, whatever sits on top.
- **Keep `ureq` linked; gate only the call sites.** Rejected. The
  reviewable property is the linked artifact, not the reachable code.
  This is the outcome the additive feature would have produced by
  accident, and it fails for the same reason.
- **Runtime kill switch (`INFERD_NO_FETCH=1`) instead of a build
  flag.** Rejected as the primary mechanism: it is exactly the
  runtime argument that does not survive the review this work exists
  to satisfy. Also rejected as a supplement — see Scope.
- **Make the airgapped build the default.** Rejected. It would break
  install=work for the overwhelming majority of users, who have
  networks, to serve the minority who do not. The minority can pass a
  flag; the majority should not have to.
- **Ship `import` only in the airgapped artifact.** Rejected. Code
  present in one artifact is code tested in one artifact, and manual
  import is useful on networked machines too.
- **Fetch via a subprocess (`curl`) so the dep tree stays clean.**
  Rejected on invariant #10 — every `Command` is a code smell, and
  inferd ships no subprocess engines. Trading a linked library for a
  spawned process is a worse security posture, not a better one.

## References

- ADR 0010 — the narrow HTTPS exception this build declines.
- ADR 0011 — CAS store layout that `inferdctl import` writes.
- ADR 0014 / 0018 — `inferdctl` as reference middleware over the same
  libraries; why `import` is a CLI concern.
- ADR 0027 — rerank as a *runtime* flag, and why build flags are
  reserved for dependency removal.
- `crates/inferd-daemon/Cargo.toml:67` — the non-optional `ureq` that
  makes an additive feature unworkable.
- `crates/inferd-daemon/src/fetch.rs:332` — the single `ureq` call
  site; the containment that makes this cheap.
- `context.md` §Invariants #8 (constant-time SHA compare), #10 (no
  subprocesses), #11 (outbound HTTPS scope).
- Issue #145 — the request, whose stated mechanism this ADR corrects.
