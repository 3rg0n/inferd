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
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)

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

    add_library(mtmd STATIC
        ${MTMD_DIR}/mtmd.cpp
        ${MTMD_DIR}/mtmd-audio.cpp
        ${MTMD_DIR}/mtmd-image.cpp
        ${MTMD_DIR}/mtmd-helper.cpp
        ${MTMD_DIR}/clip.cpp
        ${MTMD_DIR}/models/cogvlm.cpp
        ${MTMD_DIR}/models/conformer.cpp
        ${MTMD_DIR}/models/dotsocr.cpp
        ${MTMD_DIR}/models/gemma4a.cpp
        ${MTMD_DIR}/models/gemma4v.cpp
        ${MTMD_DIR}/models/glm4v.cpp
        ${MTMD_DIR}/models/granite-speech.cpp
        ${MTMD_DIR}/models/hunyuanocr.cpp
        ${MTMD_DIR}/models/internvl.cpp
        ${MTMD_DIR}/models/kimivl.cpp
        ${MTMD_DIR}/models/kimik25.cpp
        ${MTMD_DIR}/models/nemotron-v2-vl.cpp
        ${MTMD_DIR}/models/llama4.cpp
        ${MTMD_DIR}/models/llava.cpp
        ${MTMD_DIR}/models/minicpmv.cpp
        ${MTMD_DIR}/models/paddleocr.cpp
        ${MTMD_DIR}/models/pixtral.cpp
        ${MTMD_DIR}/models/qwen2vl.cpp
        ${MTMD_DIR}/models/qwen3vl.cpp
        ${MTMD_DIR}/models/mimovl.cpp
        ${MTMD_DIR}/models/qwen3a.cpp
        ${MTMD_DIR}/models/step3vl.cpp
        ${MTMD_DIR}/models/siglip.cpp
        ${MTMD_DIR}/models/whisper-enc.cpp
        ${MTMD_DIR}/models/deepseekocr.cpp
        ${MTMD_DIR}/models/mobilenetv5.cpp
        ${MTMD_DIR}/models/youtuvl.cpp
        ${MTMD_DIR}/models/yasa2.cpp
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

# llama.cpp's CMake doesn't always install ggml's archives in the
# location cmake-rs expects; force them into ${CMAKE_INSTALL_PREFIX}/lib
# so build.rs's link-search picks them up uniformly.
install(TARGETS llama ggml ggml-base ggml-cpu
    ARCHIVE DESTINATION lib
    OPTIONAL)
