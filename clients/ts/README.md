# @inferd/client (TypeScript)

TypeScript client for the inferd daemon. Published to npm.

**Status: not yet implemented.** The Go client (`clients/go/`) is the
canonical non-Rust reference; TypeScript and Python wrappers are
planned but not yet written.

Planned shape: a small `ClientV2` class exposing `generate()` that
returns an `AsyncIterable<ResponseV2>` over the v0.4 length-prefixed,
type-tagged wire (ADR 0021). Node-native net socket for Unix/pipe/TCP.
inferd has no HTTP surface (ADR 0006), so there is no `fetch` path.
