//! inferd daemon — internals exposed for integration testing.
//!
//! The shipped surface is the binary in `main.rs`. Library exports are
//! intended for tests in `tests/` and for cross-crate integration tests
//! in sibling crates; they are not a stable public API.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod endpoint;
pub mod lock;
pub mod queue;
