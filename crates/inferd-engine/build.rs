//! Build script for `inferd-engine`.
//!
//! Three paths.
//!
//! 1. Feature `llamacpp` off (default): no-op. The crate ships only
//!    the `mock` backend, which needs no native build steps.
//!
//! 2. Feature `llamacpp` on, `dl-backends` off (v0.2.x compatibility):
//!    static-everything build. Drives
//!    `crates/inferd-engine/cpp/CMakeLists.txt` (the wrapper around
//!    `vendor/llama.cpp`) to build static `libllama`, `libggml`,
//!    `libggml-base`, `libggml-cpu`, plus static `libmtmd`. One
//!    accelerator picked at compile time per the `cuda` / `metal` /
//!    `vulkan` / `rocm` cargo features. This is the v0.2.x shape.
//!
//! 3. Feature `dl-backends` on (v0.3 / ADR 0019): dynamic-loader
//!    build. `BUILD_SHARED_LIBS=ON` + `GGML_BACKEND_DL=ON` +
//!    `GGML_CPU_ALL_VARIANTS=ON` (on x86_64). `libllama` becomes a
//!    shared library; each ggml backend (cpu / metal / cuda / vulkan
//!    / hip) becomes a MODULE library that `libllama` dlopen's at
//!    runtime against what the host actually has. The daemon's
//!    accelerator probe picks the strongest available per the
//!    cascade in ADR 0019: Metal > CUDA > ROCm > Vulkan > CPU.
//!
//! Either way, generates Rust bindings from
//! `vendor/llama.cpp/include/llama.h` into
//! `OUT_DIR/llama_bindings.rs` and from
//! `vendor/llama.cpp/tools/mtmd/mtmd.h` into
//! `OUT_DIR/mtmd_bindings.rs`.
//!
//! ADR 0005 + ADR 0006 require building ONLY the inference library
//! + mtmd; server, CLIs, examples are disabled.
//!
//! See `vendor/llama.cpp.PIN.md` for the pinned commit.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_LLAMACPP");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DL_BACKENDS");

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

    // ADR 0019: dynamic-loader build. When `dl-backends` is on we flip
    // to shared libllama + MODULE backend libs. When off we stay on
    // the static-everything v0.2.x shape so existing release pipelines
    // keep working until phase 5 (tarball packaging) lands.
    let dl_backends = cfg!(feature = "dl-backends");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_x86_64 = target_arch == "x86_64";

    let mut config = cmake::Config::new(&cpp_wrapper);
    config
        // CMake build via the cpp/ wrapper. Strip every llama.cpp
        // component inferd does not consume (servers / CLIs / tests /
        // upstream tools / curl-fetch). INFERD_BUILD_MTMD is set ON
        // unconditionally for the llamacpp feature — ADR 0016 commits
        // multimodal as part of the baseline.
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_CURL", "OFF")
        .define("INFERD_BUILD_MTMD", "ON")
        // Always Release on the C++ side so the CRT matches Rust's
        // (cargo links the release CRT for both `cargo build` and
        // `cargo test`). Mixing debug-CRT C++ with release-CRT Rust
        // produces unresolved-symbol errors on Windows for *_dbg
        // helpers.
        .profile("Release");

    if dl_backends {
        // v0.3 / ADR 0019 shape.
        config
            .define("INFERD_BUILD_SHARED_LIBS", "ON")
            .define("BUILD_SHARED_LIBS", "ON")
            .define("GGML_BACKEND_DL", "ON")
            // Disable -march=native — the all-variants CPU build wants
            // each variant to be reproducible across hosts.
            .define("GGML_NATIVE", "OFF")
            // RPATH so the daemon binary finds libllama next to itself.
            // $ORIGIN on Linux, @loader_path on macOS — the cmake
            // variable accepts both, and CMAKE_BUILD_WITH_INSTALL_RPATH
            // bakes it in at link time.
            .define("CMAKE_BUILD_WITH_INSTALL_RPATH", "ON")
            .define("CMAKE_INSTALL_RPATH", "$ORIGIN");

        if is_x86_64 {
            // CPU variant matrix (sse / avx / avx2 / avx512 / amx).
            // libllama loads the strongest variant the host supports
            // at runtime via the same dl mechanism that picks
            // accelerators.
            config.define("GGML_CPU_ALL_VARIANTS", "ON");
        }

        // Per-platform backend lib enables. Each one lands in `bin/`
        // (Windows) or `lib/` (Unix) as a MODULE library with the
        // ggml-* prefix, which build.rs then ships next to the
        // daemon. Off-platform settings are no-ops.
        if cfg!(target_os = "macos") {
            config.define("GGML_METAL", "ON");
            // Metal is the only Apple Silicon path; embed the
            // shader source so libggml-metal is self-contained.
            config.define("GGML_METAL_EMBED_LIBRARY", "ON");
        }
        // CUDA / Vulkan: enabled on Linux + Windows when their cargo
        // features are on (operator opted in / CI installed the SDK).
        // A future phase will runtime-detect SDK presence and flip
        // these on automatically.
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            config.define(
                "GGML_CUDA",
                if cfg!(feature = "cuda") { "ON" } else { "OFF" },
            );
            config.define(
                "GGML_VULKAN",
                if cfg!(feature = "vulkan") {
                    "ON"
                } else {
                    "OFF"
                },
            );
        }
        // ROCm / HIP: Linux-only and only when SDK is wired (cargo
        // feature gate). Windows ROCm tooling is not stable enough.
        if cfg!(target_os = "linux") {
            config.define(
                "GGML_HIP",
                if cfg!(feature = "rocm") { "ON" } else { "OFF" },
            );
        }
    } else {
        // v0.2.x compatibility shape: static everything, single
        // accelerator picked at compile time.
        config
            .define("INFERD_BUILD_SHARED_LIBS", "OFF")
            .define("BUILD_SHARED_LIBS", "OFF")
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
            );
    }

    let dst = config.build();

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
    if cfg!(target_os = "windows") && dl_backends {
        // Windows places shared library import .libs under bin/ when
        // RUNTIME DESTINATION is bin. Add it so rustc finds llama.lib.
        println!(
            "cargo:rustc-link-search=native={}",
            dst.join("bin").display()
        );
    }

    if dl_backends {
        // Shared mode: link only libllama + libmtmd + libinferd_grammar.
        // ggml-* libs are either pulled in transitively by libllama
        // (ggml / ggml-base) or runtime-loaded modules (ggml-cpu / ggml-metal
        // / ggml-cuda / ggml-vulkan / ggml-hip).
        println!("cargo:rustc-link-lib=static=mtmd");
        println!("cargo:rustc-link-lib=static=inferd_grammar");
        println!("cargo:rustc-link-lib=dylib=llama");
        println!("cargo:rustc-link-lib=dylib=ggml");
        println!("cargo:rustc-link-lib=dylib=ggml-base");
    } else {
        // Static mode (v0.2.x). Static link order matters. mtmd + grammar
        // depend on llama + ggml so they go first; then llama; then ggml.
        println!("cargo:rustc-link-lib=static=mtmd");
        println!("cargo:rustc-link-lib=static=inferd_grammar");
        println!("cargo:rustc-link-lib=static=llama");
        println!("cargo:rustc-link-lib=static=ggml");
        println!("cargo:rustc-link-lib=static=ggml-base");
        println!("cargo:rustc-link-lib=static=ggml-cpu");
    }

    // C++ runtime. cmake-rs picks the right toolchain; we just need to
    // link the standard C++ library that llama.cpp was compiled against.
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=stdc++");
        if !dl_backends {
            // ggml-cpu compiles with OpenMP on Linux; link libgomp so
            // GOMP_barrier / GOMP_parallel etc. resolve. In dl mode
            // ggml-cpu is a separate MODULE — its OpenMP symbols are
            // self-contained.
            println!("cargo:rustc-link-lib=gomp");
        }
    } else if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=c++");
        if !dl_backends {
            // ggml on macOS compiles a BLAS backend (ggml-blas) that calls
            // vDSP_* and _ggml_backend_blas_reg from Accelerate.framework.
            println!("cargo:rustc-link-lib=static=ggml-blas");
            println!("cargo:rustc-link-lib=framework=Accelerate");
        }
    }

    // Windows-specific system libraries pulled in by ggml-cpu (registry
    // probes for CPU feature detection) and llama (mimalloc / OS heap).
    // In dl mode these resolve from inside libggml-cpu.dll's own link;
    // in static mode they have to be threaded through to the final
    // binary.
    if cfg!(target_os = "windows") && !dl_backends {
        println!("cargo:rustc-link-lib=Advapi32");
    }

    // ADR 0019 / phase 5a: stage shared+MODULE libs into a stable
    // path under target/<profile>/backends/ so release packaging
    // (the GitHub Actions release.yml staging step, the install
    // scripts under packaging/) doesn't need to know cmake-rs's
    // OUT_DIR hash. Skipped on the static-build path — there's
    // nothing to stage.
    //
    // The whole block (call + RPATH bake) is `#[cfg(feature =
    // "dl-backends")]` because `stage_backends_dir` itself is
    // feature-gated and the rustc-link-arg lines have nothing to
    // do on the static path either.
    #[cfg(feature = "dl-backends")]
    {
        stage_backends_dir(&dst, &manifest_dir);

        // ADR 0019 / phase 5d: bake `$ORIGIN` (Linux) /
        // `@loader_path` (macOS) into the final binary's RPATH so
        // libllama+ggml-* dlopen from the install dir without
        // `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH`. Windows resolves
        // co-located DLLs via the OS loader's EXE-dir-first search,
        // no equivalent flag needed.
        //
        // Phase 5d install scripts flatten the tarball's `backends/`
        // subdir into the install dir, so libllama+ggml-* end up
        // next to the daemon — matching the `$ORIGIN` /
        // `@loader_path` location.
        //
        // `cargo:rustc-link-arg` (no `-bin` suffix) applies to every
        // downstream binary that links inferd-engine. inferdctl picks
        // it up too, but inferdctl's default-features build doesn't
        // enable `dl-backends` so it won't have a libllama dep at
        // runtime — the RPATH note is harmless.
        if cfg!(target_os = "linux") {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        } else if cfg!(target_os = "macos") {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
        }
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
        // ADR 0019: also surface ggml_backend_* symbols so the runtime
        // accelerator probe (phase 3) can enumerate registered
        // backends without a second bindgen pass.
        .allowlist_function("ggml_backend_.*")
        .allowlist_type("ggml_backend_.*")
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
    // llama.h transitively, so add both include dirs. We allowlist
    // mtmd_* (the rest is already exposed via the llama bindings).
    // The same binding generation also picks up mtmd_helper_* by
    // including mtmd-helper.h alongside mtmd.h — both headers share
    // type definitions, and producing one combined output keeps the
    // module simple.
    let mtmd_header = llama_src.join("tools").join("mtmd").join("mtmd.h");
    let mtmd_helper_header = llama_src.join("tools").join("mtmd").join("mtmd-helper.h");
    let mtmd_bindings = bindgen::Builder::default()
        .header(mtmd_header.to_string_lossy())
        .header(mtmd_helper_header.to_string_lossy())
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
        .raw_line("use crate::ffi::{llama_context, llama_model, llama_pos, llama_seq_id, llama_token, llama_flash_attn_type, ggml_log_callback, ggml_backend_sched_eval_callback};")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen generate mtmd.h + mtmd-helper.h");

    mtmd_bindings
        .write_to_file(out_dir.join("mtmd_bindings.rs"))
        .expect("write mtmd bindgen output");
    println!("cargo:rerun-if-changed={}", mtmd_header.display());
    println!("cargo:rerun-if-changed={}", mtmd_helper_header.display());
}

/// Stage every shared + MODULE library produced by the cmake build
/// into `<workspace target>/<profile>/backends/`.
///
/// Why: the dl-backends release tarball (Phase 5b) and the per-OS
/// install scripts need a stable, predictable path to copy from.
/// cmake-rs places its outputs under
/// `target/<profile>/build/inferd-engine-<HASH>/out/{bin,lib}/`, and
/// the hash changes on every dependency rebuild — neither CI nor a
/// human can hard-code that path.
///
/// What gets staged: every file under `OUT_DIR/lib/` and (on Windows)
/// `OUT_DIR/bin/` whose name starts with `ggml`, `libggml`, `llama`,
/// or `libllama`. That captures the shared `libllama` itself, the
/// shared `libggml` / `libggml-base`, every CPU variant
/// (`ggml-cpu-haswell`, `ggml-cpu-skylakex`, …), and every
/// accelerator MODULE (`ggml-metal`, `ggml-cuda`, `ggml-vulkan`,
/// `ggml-hip`). We deliberately do NOT stage `.lib` import libraries
/// or `.exp` / `.pdb` files — only the runtime-loadable artefacts.
///
/// Idempotent: re-staging on every cargo build overwrites stale
/// copies. Failure to stage a file is logged via
/// `cargo:warning=` but doesn't fail the build — the static path
/// already turned this off; an absent file just means there's
/// nothing for the release to bundle.
///
/// `cargo:rustc-env=INFERD_BACKENDS_DIR=<absolute path>` is emitted
/// so consumers (smoke-test bin, future runtime probe) can find the
/// staged dir without re-deriving the same path.
#[cfg(feature = "dl-backends")]
fn stage_backends_dir(cmake_dst: &std::path::Path, manifest_dir: &std::path::Path) {
    use std::path::PathBuf;

    // OUT_DIR is target/<profile>/build/inferd-engine-<hash>/out;
    // walk up to target/<profile>/. Cargo doesn't expose this
    // directly — the climb is the only portable way.
    let out_dir = match env::var("OUT_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => return,
    };
    let target_profile_dir = match out_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    {
        Some(p) => p.to_path_buf(),
        None => {
            println!(
                "cargo:warning=stage_backends_dir: cannot derive target/<profile> from OUT_DIR={}",
                out_dir.display()
            );
            return;
        }
    };
    let backends_dir = target_profile_dir.join("backends");
    if let Err(e) = std::fs::create_dir_all(&backends_dir) {
        println!(
            "cargo:warning=stage_backends_dir: mkdir {}: {e}",
            backends_dir.display()
        );
        return;
    }

    // Source candidates: cmake puts the shared libs in `lib/` on Unix
    // and split across `bin/` (.dll) + `lib/` (.lib import libs) on
    // Windows. Sweep both; filter by name.
    let mut sources = vec![cmake_dst.join("lib")];
    if cfg!(target_os = "windows") {
        sources.push(cmake_dst.join("bin"));
    }

    for src_dir in &sources {
        let entries = match std::fs::read_dir(src_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Only runtime-loadable artefacts (skip .lib / .exp /
            // .pdb / .a). The matrix here is intentionally
            // permissive on prefix and strict on extension: ggml's
            // CPU variants ship as `ggml-cpu-haswell.dll` on
            // Windows but `libggml-cpu-haswell.so` on Linux, and
            // we want both shapes.
            let is_runtime = name.ends_with(".dll")
                || name.ends_with(".so")
                || name.ends_with(".dylib")
                || name.contains(".so."); // Linux versioned soname e.g. libllama.so.1
            if !is_runtime {
                continue;
            }
            let is_ours = name.starts_with("ggml")
                || name.starts_with("libggml")
                || name.starts_with("llama")
                || name.starts_with("libllama");
            if !is_ours {
                continue;
            }

            let dest = backends_dir.join(&name);
            if let Err(e) = std::fs::copy(&path, &dest) {
                println!(
                    "cargo:warning=stage_backends_dir: copy {} -> {}: {e}",
                    path.display(),
                    dest.display()
                );
            }
        }
    }

    // Surface the staged location for downstream consumers.
    println!(
        "cargo:rustc-env=INFERD_BACKENDS_DIR={}",
        backends_dir.display()
    );

    // Quiet the lint about manifest_dir being unused without
    // touching the call site — keeping the parameter makes the
    // signature future-proof if we later need to copy from a
    // crate-relative path (e.g. shipping a default mmproj alongside
    // the backends).
    let _ = manifest_dir;
}

#[cfg(not(feature = "llamacpp"))]
fn build_llamacpp() {
    // Reached only when the env-var check above thinks the feature is on
    // but Cargo's cfg(feature) disagrees — treat as a hard build error to
    // surface the inconsistency rather than silently skipping FFI.
    panic!("CARGO_FEATURE_LLAMACPP set but cfg(feature=\"llamacpp\") is off");
}
