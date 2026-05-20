//! Build script for `inferd-engine`.
//!
//! Two paths.
//!
//! Feature `llamacpp` off (default): no-op. The crate ships only the
//! `mock` backend, which needs no native build steps. Default `cargo
//! build` works without a C++ toolchain or `libclang`.
//!
//! Feature `llamacpp` on: drives `crates/inferd-engine/cpp/CMakeLists.txt`
//! (the wrapper around `vendor/llama.cpp`) to build `libllama`,
//! `libggml`, `libggml-base`, `libggml-cpu`, plus `libmtmd` for
//! multimodal Gemma 4 (ADR 0016 makes multimodal part of the
//! baseline `llamacpp` adapter shape). Then generates Rust bindings
//! from `vendor/llama.cpp/include/llama.h` into
//! `OUT_DIR/llama_bindings.rs` and from
//! `vendor/llama.cpp/tools/mtmd/mtmd.h` into `OUT_DIR/mtmd_bindings.rs`.
//! ADR 0005 + ADR 0006 require building ONLY the inference library
//! + mtmd; server, CLIs, examples are disabled.
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
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root resolvable");
    let llama_src = workspace_root.join("vendor").join("llama.cpp");
    let cpp_wrapper = manifest_dir.join("cpp");

    if !llama_src.join("CMakeLists.txt").exists() {
        panic!(
            "vendor/llama.cpp not populated at {}. Run \
             `git submodule update --init --recursive`.",
            llama_src.display()
        );
    }
    if !cpp_wrapper.join("CMakeLists.txt").exists() {
        panic!(
            "inferd-engine cpp wrapper not found at {}. The crate is \
             out of tree?",
            cpp_wrapper.display()
        );
    }

    println!(
        "cargo:rerun-if-changed={}",
        cpp_wrapper.join("CMakeLists.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        llama_src.join("CMakeLists.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        llama_src.join("include/llama.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        llama_src.join("tools/mtmd/mtmd.h").display()
    );

    // CMake build via the cpp/ wrapper. Strip every llama.cpp component
    // inferd does not consume:
    //   - LLAMA_BUILD_SERVER (we ship our own NDJSON server in inferd-daemon)
    //   - LLAMA_BUILD_EXAMPLES (CLIs and demos not needed)
    //   - LLAMA_BUILD_TESTS (upstream test binaries not needed)
    //   - LLAMA_BUILD_TOOLS (CLIs we don't ship; we add tools/mtmd's
    //     library target back via the cpp/ wrapper without the CLIs)
    //   - LLAMA_CURL (curl-based model fetch not needed; inferd does its
    //     own SHA-256-verified download)
    //
    // INFERD_BUILD_MTMD is set ON unconditionally for the llamacpp
    // feature — ADR 0016 commits multimodal as part of the baseline.
    let dst = cmake::Config::new(&cpp_wrapper)
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_CURL", "OFF")
        .define("INFERD_BUILD_MTMD", "ON")
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

    // Static link order matters. mtmd depends on llama + ggml so it
    // goes first; then llama; then ggml.
    println!("cargo:rustc-link-lib=static=mtmd");
    println!("cargo:rustc-link-lib=static=llama");
    println!("cargo:rustc-link-lib=static=ggml");
    println!("cargo:rustc-link-lib=static=ggml-base");
    println!("cargo:rustc-link-lib=static=ggml-cpu");

    // C++ runtime. cmake-rs picks the right toolchain; we just need to
    // link the standard C++ library that llama.cpp was compiled against.
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=stdc++");
        // ggml-cpu compiles with OpenMP on Linux; link libgomp so
        // GOMP_barrier / GOMP_parallel etc. resolve.
        println!("cargo:rustc-link-lib=gomp");
    } else if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=c++");
        // ggml on macOS compiles a BLAS backend (ggml-blas) that calls
        // vDSP_* and _ggml_backend_blas_reg from Accelerate.framework.
        println!("cargo:rustc-link-lib=static=ggml-blas");
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }

    // Windows-specific system libraries pulled in by ggml-cpu (registry
    // probes for CPU feature detection) and llama (mimalloc / OS heap).
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=Advapi32");
    }

    // bindgen for libllama's public C API.
    let llama_header = llama_src.join("include").join("llama.h");
    let llama_bindings = bindgen::Builder::default()
        .header(llama_header.to_string_lossy())
        .clang_arg(format!("-I{}", llama_src.join("include").display()))
        .clang_arg(format!(
            "-I{}",
            llama_src.join("ggml").join("include").display()
        ))
        // Only generate items reachable from the llama_* surface.
        .allowlist_function("llama_.*")
        .allowlist_type("llama_.*")
        .allowlist_var("LLAMA_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen generate llama.h");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    llama_bindings
        .write_to_file(out_dir.join("llama_bindings.rs"))
        .expect("write llama bindgen output");
    println!("cargo:rerun-if-changed={}", llama_header.display());

    // bindgen for libmtmd's public C API. mtmd.h includes ggml.h and
    // llama.h transitively, so add both include dirs. We only allow-
    // list mtmd_* (the rest is already exposed via the llama
    // bindings).
    let mtmd_header = llama_src.join("tools").join("mtmd").join("mtmd.h");
    let mtmd_bindings = bindgen::Builder::default()
        .header(mtmd_header.to_string_lossy())
        .clang_arg(format!(
            "-I{}",
            llama_src.join("tools").join("mtmd").display()
        ))
        .clang_arg(format!("-I{}", llama_src.join("include").display()))
        .clang_arg(format!(
            "-I{}",
            llama_src.join("ggml").join("include").display()
        ))
        .allowlist_function("mtmd_.*")
        .allowlist_type("mtmd_.*")
        .allowlist_var("MTMD_.*")
        // Block the llama_* / ggml_* types so they don't redefine
        // symbols that already came from `llama_bindings.rs`.
        .blocklist_type("llama_.*")
        .blocklist_type("ggml_.*")
        .blocklist_function("llama_.*")
        .blocklist_function("ggml_.*")
        .raw_line("use crate::ffi::{llama_model, llama_pos, llama_token, llama_flash_attn_type, ggml_log_callback, ggml_backend_sched_eval_callback};")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen generate mtmd.h");

    mtmd_bindings
        .write_to_file(out_dir.join("mtmd_bindings.rs"))
        .expect("write mtmd bindgen output");
    println!("cargo:rerun-if-changed={}", mtmd_header.display());
}

#[cfg(not(feature = "llamacpp"))]
fn build_llamacpp() {
    // Reached only when the env-var check above thinks the feature is on
    // but Cargo's cfg(feature) disagrees — treat as a hard build error to
    // surface the inconsistency rather than silently skipping FFI.
    panic!("CARGO_FEATURE_LLAMACPP set but cfg(feature=\"llamacpp\") is off");
}
