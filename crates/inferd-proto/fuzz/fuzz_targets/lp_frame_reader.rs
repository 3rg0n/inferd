//! Fuzz target for `inferd_proto::read_lp_frame`.
//!
//! Feeds arbitrary bytes to the v0.4 length-prefixed frame reader
//! (ADR 0021). The bounded reader must, for any input, either return a
//! parsed `RawFrame`, return `Ok(None)` on clean EOF, or return a
//! `ProtoError` (FrameTooLarge / MalformedFrame / Io / Decode) —
//! never panic, and never allocate past `MAX_FRAME_BYTES`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    // Drain frames until EOF or error; each call is independently
    // bounded, so a long input just means more iterations.
    loop {
        match inferd_proto::read_lp_frame(&mut cursor) {
            Ok(Some(_frame)) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }
});
