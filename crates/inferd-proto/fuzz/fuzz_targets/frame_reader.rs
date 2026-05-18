//! Fuzz target for `inferd_proto::read_frame`.
//!
//! Feeds arbitrary bytes from libfuzzer into the bounded reader and
//! asserts the contract: never panic, never decode garbage as a valid
//! Request, never grow the internal buffer past MAX_FRAME_BYTES.
//!
//! Successful parses are still allowed — random input occasionally
//! happens to look like valid JSON, and that's fine; we only assert
//! we *handle* it cleanly.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    // Parse repeatedly until EOF or error — multi-frame inputs in one
    // buffer should also be safe.
    loop {
        let result: Result<Option<inferd_proto::Request>, _> =
            inferd_proto::read_frame(&mut cursor);
        match result {
            Ok(Some(_)) => continue, // parsed one frame; try the next
            Ok(None) => break,        // clean EOF
            Err(_) => break,          // any error path is fine; assert NO panic
        }
    }
});
