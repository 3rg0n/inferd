# inferd-proto fuzz harness

cargo-fuzz targets for the wire-format parser. Compiled out of band
of the workspace (separate cargo project) because `cargo-fuzz` needs
nightly Rust and `libfuzzer-sys`.

## Setup

```sh
rustup install nightly
cargo install cargo-fuzz
```

## Run

```sh
cd crates/inferd-proto/fuzz
cargo +nightly fuzz run frame_reader
cargo +nightly fuzz run request_resolve
```

Each target runs until you interrupt it. Crashes land under
`fuzz/artifacts/<target>/` with a reproducer file. Treat any crash as
a real bug — open an issue with the artifact attached.

## Targets

- `frame_reader` — exercises `inferd_proto::read_frame`. Asserts the
  bounded reader (THREAT_MODEL F-1) returns `FrameTooLarge`, a parsed
  Request, `Ok(None)` on EOF, or a clean Decode/Io error. Should
  never panic and never grow the internal buffer past
  `MAX_FRAME_BYTES`.
- `request_resolve` — exercises `Request::resolve`. JSON-decodes the
  random input as a Request, then calls resolve(). Validation logic
  must never panic; should always return either `Resolved` or
  `ProtoError::InvalidRequest`.

## CI

Fuzzing is **not** part of CI per `docs/test-strategy.md` Tier 6. It
runs on a developer's box on a schedule (or on demand when a parser
change lands). Findings open issues; they don't block PRs.
