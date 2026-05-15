//! Build script for `inferd-engine`.
//!
//! In M1 this is a no-op — the crate ships only the `mock` backend, which
//! needs no native build steps. M2a replaces this with a CMake invocation
//! against the vendored `llama.cpp` submodule when the `llamacpp` feature
//! is active. See `vendor/llama.cpp/PIN.md`.

fn main() {
    // Re-run only when the build script itself changes; nothing else to do.
    println!("cargo:rerun-if-changed=build.rs");
}
