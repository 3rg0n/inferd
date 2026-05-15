//! Backend trait and adapters for inferd.
//!
//! See ADR 0005 (engine consumed via FFI), ADR 0007 (routing), and
//! `docs/ai.internals.explained.md` for the architectural framing.
//!
//! v0.1 ships:
//! - `mock` — deterministic test double, always available.
//! - `llamacpp` — FFI to vendored `libllama` (gated behind the `llamacpp`
//!   cargo feature; lands in M2a).

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod backend;
pub mod mock;

pub use backend::{Backend, GenerateError, TokenEvent, TokenStream};
