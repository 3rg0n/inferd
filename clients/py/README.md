# inferd-py

Python client for the inferd daemon. Published to PyPI as `inferd`.

**Status: not yet implemented** — deferred to after v0.1.

Planned shape: an `async` client built on `asyncio` streams, and a
`subprocess`-backed sync wrapper for scripts. Mirror the Rust client
surface (`Client`, `GenerateRequest`, `TokenStream`).
