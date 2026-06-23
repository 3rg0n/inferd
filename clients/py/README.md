# inferd-py

Python client for the inferd daemon. Published to PyPI as `inferd`.

**Status: not yet implemented.** The Go client (`clients/go/`) is the
canonical non-Rust reference; Python and TypeScript wrappers are
planned but not yet written.

Planned shape: an `async` client built on `asyncio` streams, and a
`subprocess`-backed sync wrapper for scripts. Mirror the Rust client
surface (`ClientV2`, `RequestV2`, the streamed `ResponseV2` frames)
over the v0.4 length-prefixed, type-tagged wire (ADR 0021).
