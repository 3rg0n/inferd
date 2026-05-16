//! Build script for `inferd-engine`.
//!
//! Two paths:
//!
//! - **Feature `llamacpp` off (default)**: no-op. The crate ships only the
//!   `mock` backend, which needs no native build steps. Default `cargo
//!   build` works without a C++ toolchain or `libclang`.
//!
//! - **Feature `llamacpp` on**: build `libllama` + `libggml` from the
//!   vendored submodule at `vendor/llama.cpp`, statically link them, and
//!   generate Rust bindings from `vendor/llama.cpp/include/llama.h` into
//!   `OUT_DIR/llama_bindings.rs`. ADR 0005 + ADR 0006 require building
//!   ONLY the inference library — server / CLI / examples are disabled.
//!
//! See `vendor/llama.cpp.PIN.md` for the pinned commit.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_FEATURE_LLAMACPP").is_err() {
        // Mock-only path. Nothing to do.
        return;
    }

    build_llamacpp();
}

#[cfg(feature = "llamacpp")]
fn build_llamacpp() {
    use std::path::{Path, PathBuf};

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let llama_src = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root resolvable")
        .join("vendor")
        .join("llama.cpp");

    if !llama_src.join("CMakeLists.txt").exists() {
        panic!(
            "vendor/llama.cpp not populated at {}. Run \
             `git submodule update --init --recursive`.",
            llama_src.display()
        );
    }

    println!(
        "cargo:rerun-if-changed={}",
        llama_src.join("CMakeLists.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        llama_src.join("include/llama.h").display()
    );

    // CMake build. Strip every component inferd does not consume:
    //   - LLAMA_BUILD_SERVER (we ship our own NDJSON server in inferd-daemon)
    //   - LLAMA_BUILD_EXAMPLES (CLIs and demos not needed)
    //   - LLAMA_BUILD_TESTS (upstream test binaries not needed)
    //   - LLAMA_CURL (curl-based model fetch not needed; inferd does its
    //     own SHA-256-verified download)
    let dst = cmake::Config::new(&llama_src)
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_CURL", "OFF")
        // Static libraries to keep our final binary self-contained.
        .define("BUILD_SHARED_LIBS", "OFF")
        // Always Release on the C++ side so the CRT matches Rust's
        // (cargo links the release CRT for both `cargo build` and
        // `cargo test`). Mixing debug-CRT C++ with release-CRT Rust
        // produces unresolved-symbol errors on Windows for *_dbg
        // helpers.
        .profile("Release")
        // GPU backends opt-in via cargo features. M2a default: CPU-only.
        .define(
            "GGML_CUDA",
            if cfg!(feature = "cuda") { "ON" } else { "OFF" },
        )
        .define(
            "GGML_METAL",
            if cfg!(feature = "metal") { "ON" } else { "OFF" },
        )
        .define(
            "GGML_VULKAN",
            if cfg!(feature = "vulkan") {
                "ON"
            } else {
                "OFF"
            },
        )
        .define(
            "GGML_HIP",
            if cfg!(feature = "rocm") { "ON" } else { "OFF" },
        )
        .build();

    // Linker search paths. CMake puts artefacts in OUT_DIR/build (typical
    // cmake-rs layout) but ggml splits across subdirs; sweep both.
    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("lib").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("build").display()
    );

    // Static link order matters: llama -> ggml.
    println!("cargo:rustc-link-lib=static=llama");
    println!("cargo:rustc-link-lib=static=ggml");
    println!("cargo:rustc-link-lib=static=ggml-base");
    println!("cargo:rustc-link-lib=static=ggml-cpu");

    // C++ runtime. cmake-rs picks the right toolchain; we just need to
    // link the standard C++ library that llama.cpp was compiled against.
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=stdc++");
    } else if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=c++");
    }

    // Windows-specific system libraries pulled in by ggml-cpu (registry
    // probes for CPU feature detection) and llama (mimalloc / OS heap).
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=Advapi32");
    }

    // bindgen — generate Rust bindings for the public C API.
    let header = llama_src.join("include").join("llama.h");
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", llama_src.join("include").display()))
        .clang_arg(format!(
            "-I{}",
            llama_src.join("ggml").join("include").display()
        ))
        // Only generate items reachable from the llama_* surface; avoids
        // pulling in every C standard library type and keeps the binding
        // file small and reviewable.
        .allowlist_function("llama_.*")
        .allowlist_type("llama_.*")
        .allowlist_var("LLAMA_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen generate");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("llama_bindings.rs");
    bindings.write_to_file(&out).expect("write bindgen output");
    println!("cargo:rerun-if-changed={}", header.display());
}

#[cfg(not(feature = "llamacpp"))]
fn build_llamacpp() {
    // Reached only when the env-var check above thinks the feature is on
    // but Cargo's cfg(feature) disagrees — treat as a hard build error to
    // surface the inconsistency rather than silently skipping FFI.
    panic!("CARGO_FEATURE_LLAMACPP set but cfg(feature=\"llamacpp\") is off");
}
