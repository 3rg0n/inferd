# inferd-engine native build wrapper.
#
# This CMake project wraps `vendor/llama.cpp` (driven by build.rs) and
# selects the targets inferd-engine actually consumes:
#
#   - llama, ggml, ggml-base, ggml-cpu (always on)
#   - mtmd (multimodal support — Phase 3A onward; gated on
#     INFERD_BUILD_MTMD which build.rs flips ON when the llamacpp
#     feature is active, since ADR 0016 makes mtmd part of the
#     baseline llamacpp adapter shape)
#
# We avoid LLAMA_BUILD_TOOLS=ON because that pulls in CLIs (mtmd-cli,
# llama-mtmd-debug, gemma3-cli, qwen2vl-cli, etc.) we don't want in
# the daemon binary. Instead, we add tools/mtmd directly via
# add_subdirectory; tools/mtmd's CMakeLists.txt unconditionally
# defines the `mtmd` library + the deprecated CLI executables, but
# we ignore the executables (they're built-but-not-installed; the
# binary cost lives in tools/mtmd's `add_subdirectory`-time
# evaluation, not in inferd-daemon's link).

cmake_minimum_required(VERSION 3.21)
project(inferd_engine_native LANGUAGES C CXX)

# Force the same posture inferd's build.rs requested:
set(LLAMA_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TOOLS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_COMMON OFF CACHE BOOL "" FORCE)
set(LLAMA_CURL OFF CACHE BOOL "" FORCE)

# v0.3 / ADR 0019: GGML_BACKEND_DL requires shared libs (libllama.so /
# libllama.dylib / llama.dll). Each ggml backend (cpu / metal / cuda /
# vulkan / hip) builds as a CMake MODULE library that libllama dlopen's
# at runtime based on what the host actually has. v0.2.x used static
# everything which forced one accelerator at compile time.
#
# RPATH '$ORIGIN' (Linux) / @loader_path (macOS) means the daemon
# binary loads libllama from the directory it was launched from —
# tarballs ship daemon + libllama + backends/ as a flat bundle.
# build.rs sets BUILD_SHARED_LIBS based on the dl-backends feature.
if (NOT DEFINED INFERD_BUILD_SHARED_LIBS)
    set(INFERD_BUILD_SHARED_LIBS OFF)
endif()
set(BUILD_SHARED_LIBS ${INFERD_BUILD_SHARED_LIBS} CACHE BOOL "" FORCE)

# llama.cpp sources. Resolved relative to this CMakeLists.txt's
# location (crates/inferd-engine/cpp/) up to the workspace root,
# then into vendor/llama.cpp.
set(LLAMA_CPP_DIR "${CMAKE_CURRENT_LIST_DIR}/../../../vendor/llama.cpp"
    CACHE PATH "Path to vendored llama.cpp")

if (NOT EXISTS "${LLAMA_CPP_DIR}/CMakeLists.txt")
    message(FATAL_ERROR
        "vendor/llama.cpp not populated at ${LLAMA_CPP_DIR}. "
        "Run: git submodule update --init --recursive")
endif()

# Disable warnings-as-errors / pedantic flags that break on some
# host toolchains for upstream code we don't control.
set(LLAMA_FATAL_WARNINGS OFF CACHE BOOL "" FORCE)

# macOS: enable the Accelerate-backed BLAS backend before pulling in
# llama.cpp. With EXCLUDE_FROM_ALL, ggml-blas won't build unless
# something depends on it — we wire that dependency below after the
# add_subdirectory, using if(TARGET) so this is a no-op on other
# platforms even if the variable is ignored.
if (APPLE)
    set(GGML_BLAS ON CACHE BOOL "" FORCE)
    set(GGML_BLAS_VENDOR "Apple" CACHE STRING "" FORCE)
endif()

# Pull in llama.cpp's targets. EXCLUDE_FROM_ALL means anything we
# don't directly link doesn't get built into our staging dir.
add_subdirectory("${LLAMA_CPP_DIR}" llama.cpp EXCLUDE_FROM_ALL)

# mtmd lives under tools/mtmd. Its CMakeLists.txt defines:
#   - the `mtmd` library (what we want)
#   - CLI executables (llama-mtmd-cli, llama-mtmd-debug, plus the
#     deprecation-warning shims for llama-llava-cli / gemma3-cli /
#     minicpmv-cli / qwen2vl-cli)
# The CLIs depend on `llama-common` which we don't build. Including
# tools/mtmd directly would fail when CMake tries to resolve those
# executables' link deps.
#
# Workaround: pull in only the `mtmd` library by recreating its
# add_library call here with the same source list. When the
# upstream pin moves, this list moves with it.
if (INFERD_BUILD_MTMD)
    set(MTMD_DIR "${LLAMA_CPP_DIR}/tools/mtmd")

    # Per-model encoder sources live in tools/mtmd/models/ and churn on
    # almost every upstream bump (Gemma 4 12B "unified" added gemma4ua/
    # gemma4uv; hunyuanocr was renamed hunyuanvl; etc.). Globbing them
    # means a pin bump auto-tracks the set instead of failing with
    # "Cannot find source file" on a hand-maintained list. The usual
    # CMake-glob staleness caveat doesn't bite here: build.rs sets
    # rerun-if-changed on the submodule, so any pin bump re-runs CMake
    # configure. The stable top-level mtmd sources stay explicit.
    file(GLOB MTMD_MODEL_SOURCES CONFIGURE_DEPENDS "${MTMD_DIR}/models/*.cpp")

    add_library(mtmd STATIC
        ${MTMD_DIR}/mtmd.cpp
        ${MTMD_DIR}/mtmd-audio.cpp
        ${MTMD_DIR}/mtmd-image.cpp
        ${MTMD_DIR}/mtmd-helper.cpp
        ${MTMD_DIR}/clip.cpp
        ${MTMD_MODEL_SOURCES}
    )

    target_include_directories(mtmd PUBLIC  ${MTMD_DIR})
    target_include_directories(mtmd PRIVATE ${LLAMA_CPP_DIR})
    target_include_directories(mtmd PRIVATE ${LLAMA_CPP_DIR}/vendor)
    target_link_libraries     (mtmd PUBLIC ggml llama)
    target_compile_features   (mtmd PRIVATE cxx_std_17)

    if (NOT MSVC)
        target_compile_options(mtmd PRIVATE -Wno-cast-qual)
    endif()

    # Find Threads (mtmd uses std::thread / std::mutex).
    find_package(Threads REQUIRED)
    target_link_libraries(mtmd PRIVATE Threads::Threads)

    install(TARGETS mtmd ARCHIVE DESTINATION lib)
endif()

# inferd grammar shim: wraps json_schema_to_grammar from llama.cpp.
# Builds a static library containing only the grammar conversion logic
# and the C shim. Uses the same pattern as mtmd (extract only what we
# need, link transitively via llama + ggml, don't pull in llama-common).
add_library(inferd_grammar STATIC
    ${CMAKE_CURRENT_LIST_DIR}/string_utils.cpp
    ${CMAKE_CURRENT_LIST_DIR}/grammar_shim.cpp
    ${LLAMA_CPP_DIR}/common/json-schema-to-grammar.cpp
)

target_include_directories(inferd_grammar PUBLIC  ${CMAKE_CURRENT_LIST_DIR})
target_include_directories(inferd_grammar PRIVATE ${LLAMA_CPP_DIR}/common)
target_include_directories(inferd_grammar PRIVATE ${LLAMA_CPP_DIR}/include)
target_include_directories(inferd_grammar PRIVATE ${LLAMA_CPP_DIR}/vendor)
target_include_directories(inferd_grammar PRIVATE ${LLAMA_CPP_DIR}/vendor/nlohmann)
target_link_libraries     (inferd_grammar PUBLIC ggml llama)
target_compile_features   (inferd_grammar PRIVATE cxx_std_17)

if (NOT MSVC)
    target_compile_options(inferd_grammar PRIVATE -Wno-cast-qual)
endif()

install(TARGETS inferd_grammar ARCHIVE DESTINATION lib)

# macOS: ggml-blas is defined by the add_subdirectory above when
# GGML_BLAS=ON, but EXCLUDE_FROM_ALL means it won't compile unless
# something with a build dependency pulls it in. Wire it to ggml so
# any target that links ggml also triggers ggml-blas's build.
if (TARGET ggml-blas)
    add_dependencies(ggml ggml-blas)
endif()

# Install rules. Both static and shared modes need to resolve.
#
# - Static mode (v0.2.x default, fallback): all targets are STATIC
#   libraries; only ARCHIVE artifacts exist. cmake-rs picks them up
#   from ${CMAKE_INSTALL_PREFIX}/lib for link-time inclusion.
# - Shared mode (v0.3 / GGML_BACKEND_DL=ON): llama / ggml / ggml-base
#   become SHARED libraries (.so / .dylib on Unix → lib/, .dll on
#   Windows → bin/ + import lib in lib/). ggml-cpu / ggml-metal /
#   ggml-cuda / ggml-vulkan / ggml-hip become MODULE libraries that
#   libllama dlopen's at runtime — they go in lib/ on Unix and bin/
#   on Windows alongside the daemon binary's runtime search path.
install(TARGETS llama ggml ggml-base
    LIBRARY DESTINATION lib
    ARCHIVE DESTINATION lib
    RUNTIME DESTINATION bin
    OPTIONAL)

# ggml-cpu: in static mode it's a single STATIC archive named
# `ggml-cpu`. In DL mode + GGML_CPU_ALL_VARIANTS=ON it's split into
# multiple MODULE libraries — one per uarch — so libllama can dlopen
# the strongest variant the host supports. We enumerate the full set
# of known variant names defensively and let install(... OPTIONAL)
# silently skip the ones that don't exist on this platform / arch.
if (TARGET ggml-cpu)
    install(TARGETS ggml-cpu
        LIBRARY DESTINATION lib
        ARCHIVE DESTINATION lib
        RUNTIME DESTINATION bin
        OPTIONAL)
endif()
foreach(cpu_variant
        # x86_64 variants (Intel uarchs + AMD piledriver).
        ggml-cpu-x64
        ggml-cpu-sse42
        ggml-cpu-ivybridge
        ggml-cpu-piledriver
        ggml-cpu-sandybridge
        ggml-cpu-haswell
        ggml-cpu-skylakex
        ggml-cpu-cannonlake
        ggml-cpu-cascadelake
        ggml-cpu-icelake
        ggml-cpu-alderlake
        ggml-cpu-sapphirerapids
        # ARM variants (Apple Silicon + ARM server / Snapdragon).
        ggml-cpu-armv7
        ggml-cpu-armv8
        ggml-cpu-armv8.2-a
        ggml-cpu-armv8.6-a
        ggml-cpu-arm64
        ggml-cpu-apple
        ggml-cpu-graviton
        ggml-cpu-snapdragon
        # PowerPC / s390x — kept for completeness; build.rs only
        # exercises x86_64 + aarch64 today.
        ggml-cpu-power9
        ggml-cpu-power10
        ggml-cpu-z14
        ggml-cpu-z15)
    if (TARGET ${cpu_variant})
        install(TARGETS ${cpu_variant}
            LIBRARY DESTINATION lib
            RUNTIME DESTINATION bin
            OPTIONAL)
    endif()
endforeach()

# Install ggml-blas when it was built (macOS with Accelerate).
# if(TARGET) is used defensively so this install() is a no-op on
# platforms where ggml-blas doesn't exist.
if (TARGET ggml-blas)
    install(TARGETS ggml-blas
        LIBRARY DESTINATION lib
        ARCHIVE DESTINATION lib
        RUNTIME DESTINATION bin
        OPTIONAL)
endif()

# Backend MODULE libraries that exist only in DL mode. Each one is a
# loadable plugin libllama discovers at runtime. Install them so the
# daemon's tarball can ship them next to the binary. if(TARGET)
# makes each entry a no-op when that backend wasn't enabled.
foreach(backend ggml-metal ggml-cuda ggml-vulkan ggml-hip ggml-rpc)
    if (TARGET ${backend})
        install(TARGETS ${backend}
            LIBRARY DESTINATION lib
            RUNTIME DESTINATION bin
            OPTIONAL)
    endif()
endforeach()
