//! Fuzz target for `inferd_proto::Request::resolve`.
//!
//! Feeds arbitrary bytes interpreted as a JSON Request envelope into
//! resolve(). The validation logic must never panic — every code path
//! should return either `Resolved` or `ProtoError::InvalidRequest`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let req: inferd_proto::Request = match serde_json::from_slice(data) {
        Ok(r) => r,
        Err(_) => return, // not our problem — JSON parser handles bad bytes
    };
    let _ = req.resolve();
});
