# 0005. Consume libllama via FFI, not llamafile as a subprocess

- Status: accepted
- Date: 2026-05-15
- Supersedes: [0003](0003-subprocess-llamafile-not-ffi.md)

## Context

ADR 0003 picked the path of least resistance: spawn the
already-shipping `llamafile` binary as a subprocess and exchange
tokens over stdio. That posture treats inferd as a thin wrapper —
"the steakhouse already cooked the steak; we just plate it."

That posture is wrong for what inferd actually is.

inferd's thesis is to be the host-wide local-inference *endpoint*
for every middleware on the machine, with model routing, policy,
and a lean perimeter that other tools extend rather than embed.
A llamafile subprocess drags along components inferd does not
want to ship: an HTTP/OpenAI-compatible server, a CLI REPL,
chatbot UI scaffolding, and Cosmopolitan packaging machinery.
We pay for all of it in binary size, attack surface, and version
coupling, and we get only one thing back — the inference engine.

The right framing is the lego-block one: consume the engine as a
**library** and own everything that wraps it.

`llama.cpp` already exposes its inference engine as `libllama` —
the same library that `llama-cli` and `llama-server` link against.
The HTTP server, the CLI, and the OpenAI-compat adapter are
*consumers* of `libllama`, not part of it. Linking `libllama`
directly into inferd gives us components 1–10 of the inference
stack (per `docs/ai.internals.explained.md`) without inheriting
components 13 (HTTP transport), the CLI, or any of llamafile's
packaging. We get the engine; we leave the cooked dish behind.

## Decision

The v0.1 default backend is **`inferd-engine-llamacpp`**, an FFI
adapter that links `libllama` (vendored from `ggerganov/llama.cpp`)
directly into the inferd binary. No subprocess. No HTTP. No stdio
protocol.

Implementation path:

- Vendor `llama.cpp` as a git submodule under
  `vendor/llama.cpp/` (or depend on a maintained binding crate
  such as `llama-cpp-2` if its build flags can be constrained
  to omit the server and CLI targets).
- Build `libllama` statically via `build.rs` invoking CMake.
  Disable every llama.cpp build option that pulls in components
  inferd does not consume — explicitly: `LLAMA_BUILD_SERVER=OFF`,
  `LLAMA_BUILD_EXAMPLES=OFF`, `LLAMA_BUILD_TESTS=OFF`. GPU
  backends (CUDA, Metal, Vulkan, ROCm) are opt-in cargo features,
  off by default.
- Generate Rust bindings via `bindgen` against `llama.h`.
- Expose the engine through the existing `Backend` trait. No
  trait changes — the FFI implementation is just a different
  adapter behind the same interface as the `mock` backend.

The `inferd-engine-llamafile-subprocess` adapter that ADR 0003
implied is **not built**. It can exist later as a third-party
crate for users who already have llamafile installed, but it is
not what inferd ships.

## Consequences

**Why this is the right call:**

- One binary. No `Command::spawn`, no stdio protocol, no
  subprocess restart supervisor. The "every `exec::Command` is
  a code smell" invariant becomes vacuous — there are no
  subprocesses left to review.
- No loopback HTTP, anywhere. The transport stack inferd ships
  is NDJSON over UDS / named pipe / loopback TCP, by operator
  choice. llamafile's HTTP server is never compiled.
- Zero added latency from subprocess IPC. Direct function calls,
  direct token callbacks into Rust streaming code.
- We control which kernels and which GPU backends get compiled.
  Per `docs/ai.internals.explained.md` §5, a llama.cpp build
  with no GPU flags is CPU-only and small; opt-in CUDA on
  Linux/Windows and Metal on macOS are cargo features.
- The `Backend` trait is unchanged, so the mock backend, the
  future Anthropic adapter, and any future Rust-native engine
  (candle, mistral.rs) all coexist cleanly.

**What we take on:**

- A C++ toolchain in CI for every target platform we ship. CMake
  + a working C++17 compiler + `bindgen`'s libclang dependency.
  This is real complexity but it is bounded and well-trodden.
- We pin to a specific llama.cpp commit. Bumping it is a real
  change with a real PR — read the upstream changelog, run the
  full integration suite, ship.
- A crash inside `libllama` crashes the daemon. Mitigated by
  feeding it only validated inputs (per the proto crate), the
  64 MiB frame cap, and the fact that we control exactly when
  and how it is invoked. Subprocess isolation was a hedge
  against an engine we did not control; we control this one
  by linking it.

**What we explicitly do not take on:**

- Reimplementing `llama.cpp` in Rust. ADR 0005 *consumes* the
  engine; it does not *replace* it. Components 1–10 of the
  inference stack remain ggerganov's work, vendored, pinned,
  and credited.
- Maintaining a fork. We patch only if upstream rejects a fix
  we need; first preference is always upstreaming.

## Alternatives considered

- **Stay subprocess (ADR 0003).** Already addressed above.
  Treats inferd as a wrapper around a finished product. Drags
  in HTTP, CLI, and packaging we do not want.
- **Pure Rust engine (`candle` or `mistral.rs`).** Attractive
  for the "no C++" story, but at 2026-05-15 neither ships
  grammar-constrained sampling (GBNF) at parity with llama.cpp.
  thlibo and its successors depend on GBNF for structured
  output. Not viable as a v0.1 default. Stays on the table as
  an additional `Backend` adapter when a Rust-native engine
  ships GBNF.
- **Hand-rolled forward pass in Rust.** Years of work for one
  architecture, multiplied per CPU ISA and GPU backend. Out
  of scope. Not what inferd is.

## When this gets revisited

When a Rust-native engine reaches feature parity with
`llama.cpp` for the workloads inferd serves — specifically:
GGUF loading, K-quants, GBNF, multimodal text+vision+audio,
and at least one accelerated GPU backend. At that point, write
a successor ADR proposing a second backend adapter alongside
the FFI one (not replacing it).

## References

- `docs/ai.internals.explained.md` — the 15-component model
  that frames what we are consuming versus what we are owning.
- `ggerganov/llama.cpp` — the engine we vendor.
- ADR 0001 — wire protocol stays frozen; this decision is
  invisible to clients.
- ADR 0006 — lean-core posture; HTTP is one of the things this
  ADR is rejecting from the daemon perimeter.
