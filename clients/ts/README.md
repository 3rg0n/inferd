# @inferd/client (TypeScript)

TypeScript client for the inferd daemon. Published to npm.

**Status: not yet implemented** — deferred to after v0.1.

Planned shape: a small `Client` class exposing `generate()` that
returns an `AsyncIterable<TokenFrame>`. Node-native net socket for
Unix/TCP, and `fetch`-over-HTTP if inferd ever adds an HTTP bridge.
