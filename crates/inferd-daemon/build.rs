// Build script for inferd-daemon.
//
// The only job here is to bake an RPATH into the daemon binary when the
// `dl-backends` feature is active so libllama.dylib / libllama.so can be
// found next to the daemon at runtime without setting LD_LIBRARY_PATH /
// DYLD_LIBRARY_PATH.
//
// Why here and not in inferd-engine's build.rs:
// `cargo:rustc-link-arg` emitted by a library crate's build script does
// NOT propagate to downstream binaries — only `rustc-link-search` and
// `rustc-link-lib` propagate.  The daemon binary therefore needs its own
// build script to emit the linker flag that lands in the final link.

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DL_BACKENDS");

    if std::env::var("CARGO_FEATURE_DL_BACKENDS").is_err() {
        return;
    }

    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");

    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/backends");

    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");

    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/backends");
}
