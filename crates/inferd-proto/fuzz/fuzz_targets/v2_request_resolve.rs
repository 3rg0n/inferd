//! Fuzz target for `inferd_proto::v2::RequestV2::resolve`.
//!
//! Feeds arbitrary bytes interpreted as a JSON v2 Request envelope into
//! resolve(). The validation logic must never panic — every code path
//! should return either a `ResolvedV2` or a `ProtoError::InvalidRequest`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let req: inferd_proto::v2::RequestV2 = match serde_json::from_slice(data) {
        Ok(r) => r,
        Err(_) => return, // not valid JSON for this envelope; nothing to resolve
    };
    let _ = req.resolve();
});
