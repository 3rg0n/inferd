//! Backend trait and adapters for inferd.
//!
//! See ADR 0005 (engine consumed via FFI), ADR 0007 (routing), and
//! `docs/ai.internals.explained.md` for the architectural framing.
//!
//! v0.1 ships:
//! - `mock` — deterministic test double, always available.
//! - `llamacpp` — FFI to vendored `libllama` (gated behind the `llamacpp`
//!   cargo feature; lands in M2a).

// `deny` rather than `forbid` so the FFI module (M2a, gated behind the
// `llamacpp` feature) can scope an inner `#![allow(unsafe_code)]` to the
// generated bindings. Every other module in the crate is unsafe-free; CI
// `cargo deny`/clippy lint surfaces any regression.
#![deny(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod backend;
#[cfg(feature = "llamacpp")]
pub(crate) mod ffi;
#[cfg(feature = "llamacpp")]
pub mod llamacpp;
pub mod mock;

pub use backend::{Backend, GenerateError, TokenEvent, TokenStream};
