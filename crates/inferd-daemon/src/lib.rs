//! inferd daemon — internals exposed for integration testing.
//!
//! The shipped surface is the binary in `main.rs`. Library exports are
//! intended for tests in `tests/` and for cross-crate integration tests
//! in sibling crates; they are not a stable public API.

// `deny` rather than `forbid` so the platform-specific peercred
// submodules can scope an inner `#![allow(unsafe_code)]` for the
// libc/Win32 FFI surface needed to read SO_PEERCRED /
// GetNamedPipeClientProcessId. Every other module remains
// unsafe-free.
#![deny(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod admin;
pub mod auth;
pub mod config;
pub mod config_file;
pub mod endpoint;
pub mod fetch;
pub mod lifecycle;
pub mod lock;
pub mod logx;
pub mod peercred;
pub mod queue;
pub mod redact;
pub mod router;
pub mod status;
pub mod store;
#[cfg(windows)]
pub mod windows_security;
