# inferd-go

Go client for the inferd daemon. Submodule of this monorepo at
`github.com/3rg0n/inferd/clients/go`.

**Status: not yet implemented** — milestone M5. Hand-written Rust-to-
Go translation of `inferd-proto`, plus a `Client` wrapper that
connects to the platform-default endpoint.

Once this crate exists, thlibo v0.2 deletes its `internal/daemon/`
and `internal/ipc/` packages and imports this module instead.
