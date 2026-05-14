# 0002. Rust, not Go

- Status: accepted
- Date: 2026-05-14

## Context

thlibo is written in Go and its embedded daemon works well. The
obvious path of least resistance for a "just split out the daemon"
project would be to stay in Go and share types directly.

inferd's mission, however, is broader than thlibo's daemon:

- It becomes host-wide infrastructure for every local AI middleware.
- It streams large token volumes (v0.2 will multiplex N middlewares
  through one model, which multiplies the GC pressure).
- It grows toward a model-proxy-gateway role where the backend
  adapter trait has to hold disparate clients (local subprocess,
  cloud HTTP, future gRPC) behind one interface.
- It will want to ship as a small static binary with no runtime
  surprises.

## Decision

Rust.

## Consequences

**Why this works for inferd specifically:**

- Zero-cost async over a single `tokio` runtime gives us tight
  control over streaming without GC pauses.
- The `Backend` trait is a natural fit for Rust's trait system; the
  equivalent in Go would be an interface plus dynamic dispatch plus
  runtime type assertions for per-backend config.
- `tokio`, `hyper`, `reqwest`, `serde` are batteries-included for
  the cloud-adapter work that's coming in v0.2.
- The binary is a single static artefact via `musl` on Linux and the
  default on Windows/macOS.
- `cargo audit` + `cargo deny` are industry-standard; the Go
  equivalents (`govulncheck`, gomodguard) are solid too, so this is
  a wash.

**Cost:**

- The one-time cost of porting the daemon logic from Go to Rust.
- Cross-language client story: thlibo stays in Go, so we need either
  a hand-written Go client that talks NDJSON (easy, ~400 lines) or a
  code-generation step from the wire-protocol schema. v0.1 hand-
  writes; v0.2 considers codegen.
- The reviewer population for thlibo-side PRs and inferd-side PRs is
  now different. Documentation has to work harder.

## Alternatives considered

- **Stay in Go.** Rejected because of the streaming-multiplex
  motivation and because cloud backends will pull in more HTTP
  plumbing than Go's stdlib comfortably covers.
- **Zig.** Attractively small but the async story is immature and
  the ecosystem for HTTP clients doesn't match what the cloud
  adapters need.
- **C++/C.** Too much manual lifetime management for a project that
  will touch tokens, streams, and async handles constantly.

## References

- thlibo's `internal/daemon/lifecycle.go` — the Go version we are
  porting semantics from.
- context.md — full hand-off brief for the implementing engineer.
