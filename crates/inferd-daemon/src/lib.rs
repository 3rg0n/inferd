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

/// Which release artifact this binary was built as (ADR 0028).
///
/// `"networked"` for the default build; `"airgapped"` when built with
/// `--no-default-features`, which drops the `model-fetch` feature and
/// with it the whole `ureq`/`rustls` tree.
///
/// `--no-default-features` is a sharp edge — someone stripping a
/// *different* default in a future release would silently lose fetch —
/// so both binaries name the profile in `--version` and the daemon logs
/// it at boot. An operator who has lost track of which archive they
/// installed can ask the process instead of guessing from a filename.
pub const BUILD_PROFILE: &str = if cfg!(feature = "model-fetch") {
    "networked"
} else {
    "airgapped"
};

/// `clap`'s `--version` (long form) for both shipped binaries: the
/// version plus the [`BUILD_PROFILE`] and what it implies.
///
/// A `const` built with `concat!` rather than a formatted `String` so
/// the text is fixed at compile time by the same `cfg` that decides
/// whether the HTTPS stack is linked — the two cannot disagree.
///
/// `inferdctl` reuses this const. Its own package version is identical
/// by construction: every internal dep is pinned `=X.Y.Z` to the single
/// workspace version.
#[cfg(feature = "model-fetch")]
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nbuild profile: networked (model-fetch on — ADR 0010 HTTPS model bootstrap available)"
);

/// Airgapped variant of [`LONG_VERSION`]; see that const for the rationale.
#[cfg(not(feature = "model-fetch"))]
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nbuild profile: airgapped (model-fetch off — no HTTPS client linked; \
     load models with `inferdctl import`)"
);

pub mod admin;
pub mod autoselect;
pub mod config;
pub mod config_file;
pub mod endpoint;
pub mod fetch;
pub mod lifecycle;
pub mod lifecycle_embed;
pub mod lifecycle_rerank;
pub mod lifecycle_v2;
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
