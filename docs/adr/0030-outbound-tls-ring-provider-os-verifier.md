# 0030. Outbound TLS: rustls on `ring`, certificate chains verified by the OS

- Status: accepted
- Date: 2026-10-08

## Context

inferd makes outbound HTTPS in exactly two places. The daemon fetches
one pinned model file on first boot (ADR 0010) through `ureq`; the
feature-gated cloud adapters (`openai-compat`, `bedrock-invoke`) talk to
their upstream through `reqwest`. The airgapped build links neither
(ADR 0028).

Until v0.8 the two clients were configured differently, and implicitly:
`ureq` 2 with `native-certs` (rustls, `ring`, roots loaded from the OS
store), `reqwest` 0.12 with `rustls-tls` (rustls, `ring`, the bundled
Mozilla root set from `webpki-roots`). Nobody chose that split; each was
a crate default.

The next major versions of both crates move those defaults, and neither
can be upgraded without choosing:

- **`reqwest` 0.13**'s `rustls` feature hard-wires the **aws-lc-rs**
  provider and switches trust to `rustls-platform-verifier`. aws-lc-rs
  builds `aws-lc-sys` — C plus CMake (and NASM on Windows) — a second
  native toolchain dependency and a second crypto library beside the
  `ring` the daemon already links. Its `rustls-no-provider` feature
  avoids that, but then panics when a `Client` is built unless a
  process-wide `CryptoProvider` was installed — a global a library
  crate has no business owning.
- **`ureq` 3** drops `native-certs`. It offers the bundled Mozilla roots
  (`rustls-webpki-roots`) or the OS verifier (`platform-verifier`);
  neither reproduces "OS roots, webpki verification" exactly.

## Decision

Both clients use **rustls on the `ring` provider**, and delegate
certificate-chain verification to the **operating system** via
`rustls-platform-verifier`.

- `ureq` 3 (`crates/inferd-daemon/src/fetch.rs`): `TlsProvider::Rustls`
  with `RootCerts::PlatformVerifier`.
- `reqwest` 0.13 (`crates/inferd-engine/src/tls.rs`): built with
  `rustls-no-provider`, and handed a complete `rustls::ClientConfig` via
  `tls_backend_preconfigured` — `ring` provider, OS verifier, HTTP/1.1
  ALPN. Both adapters build their client through this one function, so
  they cannot drift apart again. No process-global provider is
  installed.

aws-lc-rs is not linked by any build. The airgapped CI denylist names
it, so a dependency that starts pulling it in fails the build.

## Consequences

- **Trust follows the host.** On Windows (CertGetCertificateChain) and
  macOS (SecTrust) the OS evaluates the chain, honouring
  enterprise-installed roots and distrust decisions. On Windows the
  verifier also checks the leaf's revocation status, soft-failing when
  revocation data cannot be fetched. On
  Linux the verifier reads the system root store, which is what the
  model fetch already did. For the cloud adapters this is a real change
  from the bundled Mozilla set: a corporate TLS-inspecting proxy whose
  root is installed system-wide now works, and a root the OS has
  removed is no longer trusted just because an old `webpki-roots`
  still carried it.
- **The model fetch's security control is unchanged.** ADR 0010's
  control is the pinned SHA-256, not TLS. TLS still protects
  confidentiality and the redirect chain; a wrong file is rejected by
  the digest whichever roots signed the connection.
- **One crypto library.** `ring` only, no second native build.
  `webpki-roots` is still linked — `ureq`'s `rustls` feature pulls it
  in — but is not consulted while `RootCerts::PlatformVerifier` is set.
- **Behaviour kept across the `ureq` 3 rewrite:** non-2xx stays a
  `FetchError::HttpStatus` (`http_status_as_error(false)`), redirects
  are capped at ureq 2's 5 rather than ureq 3's 10, and the body is read
  uncapped (the 10 MB limit applies only to `read_to_*`).
- A Linux host with no readable system root store can no longer reach
  a cloud upstream: the adapters fail at construction with `TlsSetup`
  ("No CA certificates were loaded from the system") rather than falling
  back to the bundled set, and the model fetch fails its connection.
  That is the intended direction: no trust is better than unexamined
  trust.

## Alternatives considered

- **Accept the new defaults** (aws-lc-rs for `reqwest`, bundled roots or
  OS verifier for `ureq`): two crypto libraries and a CMake build
  dependency for no capability inferd uses.
- **Bundled Mozilla roots for both** (`webpki-roots`): uniform, but
  ignores the host's trust decisions and pins revocation to whatever
  shipped in the binary.
- **Stay on `ureq` 2 / `reqwest` 0.12**: defers the choice while both
  lines stop receiving fixes.

## References

- ADR 0010 — narrow HTTPS exception for model bootstrap.
- ADR 0028 — airgapped build profile.
- `crates/inferd-engine/src/tls.rs`, `crates/inferd-daemon/src/fetch.rs`.
