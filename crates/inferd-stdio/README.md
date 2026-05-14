# inferd-stdio

Variant of the daemon that speaks NDJSON on stdin/stdout instead of
binding a socket. Use when:

- Running inside a CI runner that forbids IPC sockets.
- Embedding inside another subprocess pipeline.
- Debugging the protocol without setting up the full listener.

Shares 100% of the request-handling code with `inferd-daemon`; the
only difference is the transport layer.

**Status: not yet implemented** — milestone M4.
