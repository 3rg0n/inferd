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

    // Windows + CUDA: drive CMake with the **Ninja** generator instead of
    // the default Visual Studio generator. The VS generator compiles
    // `.cu` files through MSBuild, which requires CUDA's
    // `visual_studio_integration` `.props`/`.targets` — and those are
    // version-matched to a *specific* VS release. CUDA 12.6 only ships VS
    // 2022 integration, so on a newer image (windows-2025 → VS 2026)
    // `enable_language(CUDA)` fails "No CUDA toolset found" (the #162
    // drift). Ninja sidesteps MSBuild entirely: `nvcc` invokes `cl.exe`
    // directly, so the build is VS-version-agnostic and the runner image
    // no longer has to be pinned to match the CUDA toolkit. Requires
    // `cl.exe` + `ninja` on PATH (the MSVC dev environment) — release.yml
    // enters it via ilammas/msvc-dev-cmd before the build. Only the
    // CUDA build needs this; the non-CUDA Windows path keeps the default
    // generator (no nvcc, no MSBuild-integration dependency).
    let win_cuda = cfg!(all(target_os = "windows", feature = "cuda"));

    // Windows + arm64: ggml's CPU backend hard-refuses MSVC on ARM
    // (`ggml-cpu/CMakeLists.txt`: "MSVC is not supported for ARM, use
    // clang" — it guards on `MSVC AND NOT CMAKE_C_COMPILER_ID == Clang`).
    // The default VS generator drives `cl.exe`, which trips that guard
    // and fails configure. Fix: Ninja generator + `clang-cl` for C/C++.
    // clang-cl is LLVM's MSVC-compatible driver, so it satisfies ggml's
    // "use clang" requirement (CMAKE_C_COMPILER_ID == Clang) while staying
    // MSVC-ABI-compatible — it links cleanly against the MSVC runtime that
    // Rust's *-pc-windows-msvc target uses. release.yml installs a native
    // arm64 LLVM (clang-cl on PATH) for this target. There is no CUDA on
    // Windows arm64, so this path never overlaps with win_cuda.
    let win_arm64 = cfg!(all(target_os = "windows", target_arch = "aarch64"));
    if win_cuda || win_arm64 {
        config.generator("Ninja");
    }
    if win_arm64 {
        // Force clang-cl for both the CMake define AND the CC/CXX env.
        // The LLVM-install action exports CC=clang / CXX=clang++ (plain
        // clang, GNU-style driver) with `env: true`, and cmake-rs
        // forwards CC/CXX into the CMake invocation — which would fight
        // the CMAKE_*_COMPILER define and/or pick plain clang (wrong
        // driver for the MSVC target). Setting both here makes clang-cl
        // unambiguous: the define wins for CMake's own compiler ID probe,
        // and overriding the env stops cmake-rs from re-injecting clang.
        config
            .define("CMAKE_C_COMPILER", "clang-cl")
            .define("CMAKE_CXX_COMPILER", "clang-cl")
            .env("CC", "clang-cl")
            .env("CXX", "clang-cl");
        // clang-cl on arm64 does not enable C++ exceptions by default the
        // way cl.exe does, but ggml's gguf.cpp uses try/throw ("cannot use
        // 'try' with exceptions disabled"). Pass /EHsc explicitly so the
        // clang-cl driver enables the standard MSVC exception model.
        // cmake-rs appends these to CMAKE_CXX_FLAGS for the configure.
        config.cxxflag("/EHsc");
    }

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
        // These llama_*/ggml_* types are blocklisted above (they come from
        // llama_bindings.rs) but mtmd.h references them in fn signatures, so
        // import them from crate::ffi. `llama_batch` was added in the b9850
        // bump — mtmd.h's new callback type takes a `llama_batch`.
        .raw_line("use crate::ffi::{llama_batch, llama_context, llama_model, llama_pos, llama_seq_id, llama_token, llama_flash_attn_type, ggml_log_callback, ggml_backend_sched_eval_callback};")
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

    // Windows arm64 only: stage LLVM's OpenMP runtime (`libomp.dll`)
    // next to the ggml/llama DLLs.
    //
    // As of llama.cpp b9850 (commit 826539ce5, "ggml: Parallelize quant
    // LUT init") OpenMP is linked into **ggml-base** — the core lib every
    // backend loads — not just the CPU backend. `GGML_OPENMP` defaults ON.
    // On x86_64 Windows the build uses MSVC (`cl.exe`), so OpenMP resolves
    // to `vcomp140.dll`, a Visual C++ redistributable already present on
    // the machine — nothing to stage. On **arm64** Windows ggml refuses
    // MSVC and the build uses **clang-cl** (see `win_arm64` in
    // `build_llamacpp`), whose OpenMP runtime is LLVM's `libomp.dll`. That
    // DLL lives only in the LLVM install dir, so `llama.dll` (→ ggml-base)
    // fails to load with `0xC0000135` unless `libomp.dll` sits next to the
    // exe. This mirrors how the Linux CUDA path bundles the CUDA runtime
    // libs next to `libggml-cuda.so`: a bundled module's non-system
    // runtime dependency travels with it.
    //
    // Gate on the *target* (CARGO_CFG_TARGET_*), NOT `cfg!(...)` — a build
    // script is compiled for the HOST, so `cfg!(target_arch)` would read
    // the host arch and misfire under cross-compilation. The rest of this
    // build script uses the same CARGO_CFG_TARGET_* convention (see the
    // `target_arch` binding in `build_llamacpp`).
    let libomp_target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let libomp_target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if libomp_target_os == "windows" && libomp_target_arch == "aarch64" {
        stage_libomp_windows_arm64(&backends_dir);
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

/// Copy LLVM's `libomp.dll` into `backends_dir` on Windows arm64.
///
/// See the call site for why this is needed (b9850 links OpenMP into
/// ggml-base; the clang-cl arm64 build's OpenMP runtime is `libomp.dll`,
/// which isn't a system DLL). We locate it via `LLVM_PATH` (exported by
/// the CI `install-llvm-action`) or `LIBCLANG_PATH` (bindgen already
/// needs it, so it's set on this target), falling back to any `bin`
/// directory of an `llvm` install on `PATH`. If it can't be found the
/// build still succeeds — a `cargo:warning` fires and the release job's
/// arm64 `--help` verify step catches the missing DLL loudly rather than
/// shipping a broken tarball.
///
/// Called only when the *target* is windows-aarch64 (guarded at the call
/// site via `CARGO_CFG_TARGET_*`), but not itself `cfg`-gated: a build
/// script compiles for the host, so a `cfg(target_arch)` attribute here
/// would exclude the function whenever the host isn't arm64 — including a
/// cross-compile to windows-arm64 from an x64 host. `allow(dead_code)`
/// because the only call is behind a runtime env-var check the compiler
/// can't see through, so on any non-arm64-windows host build it looks
/// unused even though it is reachable on the target that needs it.
#[allow(dead_code)]
fn stage_libomp_windows_arm64(backends_dir: &std::path::Path) {
    use std::path::PathBuf;

    if backends_dir.join("libomp.dll").exists() {
        return; // already staged (e.g. incremental rebuild)
    }

    // Candidate directories that may hold libomp.dll, in priority order.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for var in ["LLVM_PATH", "LIBCLANG_PATH"] {
        if let Ok(v) = env::var(var) {
            let p = PathBuf::from(&v);
            // LLVM_PATH is the install root (…/bin holds the dll);
            // LIBCLANG_PATH may already point at …/bin or …/lib.
            candidates.push(p.join("bin"));
            candidates.push(p.clone());
            if let Some(parent) = p.parent() {
                candidates.push(parent.join("bin"));
            }
        }
    }

    for dir in &candidates {
        let src = dir.join("libomp.dll");
        if src.is_file() {
            let dest = backends_dir.join("libomp.dll");
            match std::fs::copy(&src, &dest) {
                Ok(_) => {
                    println!(
                        "cargo:warning=staged OpenMP runtime {} -> {}",
                        src.display(),
                        dest.display()
                    );
                    return;
                }
                Err(e) => {
                    println!(
                        "cargo:warning=stage_libomp: copy {} -> {}: {e}",
                        src.display(),
                        dest.display()
                    );
                }
            }
        }
    }

    println!(
        "cargo:warning=stage_libomp: libomp.dll not found (searched {} candidate dir(s) via \
         LLVM_PATH/LIBCLANG_PATH). The Windows arm64 daemon links OpenMP into ggml-base \
         (llama.cpp b9850+); without libomp.dll next to the exe it fails to load (0xC0000135). \
         Set LLVM_PATH to the LLVM install root.",
        candidates.len()
    );
}

#[cfg(not(feature = "llamacpp"))]
fn build_llamacpp() {
    // Reached only when the env-var check above thinks the feature is on
    // but Cargo's cfg(feature) disagrees — treat as a hard build error to
    // surface the inconsistency rather than silently skipping FFI.
    panic!("CARGO_FEATURE_LLAMACPP set but cfg(feature=\"llamacpp\") is off");
}
