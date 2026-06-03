//! Runtime accelerator detection (ADR 0019).
//!
//! With `dl-backends` on, libllama is built with `GGML_BACKEND_DL=ON` and
//! each ggml backend (CPU / Metal / CUDA / Vulkan / HIP) is a MODULE
//! library libllama dlopen's at runtime. The set of backends actually
//! registered is therefore a property of *the host the daemon is
//! running on*, not of the build. This module probes that set on first
//! call, picks the strongest available accelerator per the cascade in
//! ADR 0019 (Metal > CUDA > ROCm > Vulkan > CPU), and caches the
//! result for the lifetime of the process.
//!
//! Without `dl-backends` (the v0.2.x static-build path) the backend is
//! pinned at compile time — see [`compile_time_accelerator_kind`] in
//! the parent module. This module is unused on that path.
//!
//! Operator override: `INFERD_FORCE_BACKEND` accepts one of `cpu`,
//! `metal`, `cuda`, `rocm`, `vulkan` (case-insensitive). When set and
//! valid, the probe returns the requested kind unconditionally —
//! useful for forcing CPU on a GPU host for benchmarking, or
//! sanity-checking that a particular accelerator is actually loadable
//! before relying on the auto-pick.

#![allow(unsafe_code)] // FFI surface; module-scoped.

use crate::backend::AcceleratorKind;
use crate::ffi;
use std::ffi::CStr;
use std::sync::OnceLock;

/// Optional per-device detail that the registry exposes once
/// `ggml_backend_load_all` has run. Empty fields mean "not available
/// from this backend" and should surface as `None` to the caller.
#[derive(Debug, Default, Clone)]
pub(super) struct DeviceDetails {
    pub name: Option<String>,
    pub total_bytes: Option<u64>,
}

/// Cached probe result. Probing is idempotent and `ggml_backend_load_all`
/// is process-wide, so caching once is correct.
static PROBE: OnceLock<AcceleratorKind> = OnceLock::new();

/// Returns the strongest accelerator the running host actually has.
///
/// The first caller drives `ggml_backend_load_all()` and the
/// enumeration; subsequent callers get the cached value without
/// re-probing. Honors `INFERD_FORCE_BACKEND` for operator-driven
/// override.
pub(super) fn probe_accelerator() -> AcceleratorKind {
    *PROBE.get_or_init(probe_accelerator_uncached)
}

fn probe_accelerator_uncached() -> AcceleratorKind {
    if let Some(forced) = forced_kind() {
        tracing::info!(
            kind = forced.as_str(),
            "INFERD_FORCE_BACKEND override applied; skipping auto-detection"
        );
        return forced;
    }

    // SAFETY: FFI; documented as idempotent + process-wide. Loads the
    // ggml-* MODULE libraries that ship next to the daemon binary
    // (search path: $ORIGIN on Unix via RPATH, the directory of the
    // executable on Windows via the OS loader).
    unsafe { ffi::ggml_backend_load_all() };

    let registered = enumerate_registered_backends();
    let pick = pick_from_cascade(&registered);
    tracing::info!(
        registered = ?registered,
        chosen = pick.as_str(),
        "runtime accelerator probe complete"
    );
    pick
}

/// Read `INFERD_FORCE_BACKEND` and parse it.
///
/// Unknown values produce a warning and fall through to auto-detect —
/// rather than failing or panicking — so a typo doesn't bring the
/// daemon down. Empty / unset returns `None`.
fn forced_kind() -> Option<AcceleratorKind> {
    let raw = std::env::var("INFERD_FORCE_BACKEND").ok()?;
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    match v.to_ascii_lowercase().as_str() {
        "cpu" => Some(AcceleratorKind::Cpu),
        "metal" => Some(AcceleratorKind::Metal),
        "cuda" => Some(AcceleratorKind::Cuda),
        "rocm" | "hip" => Some(AcceleratorKind::Rocm),
        "vulkan" | "vk" => Some(AcceleratorKind::Vulkan),
        _ => {
            tracing::warn!(
                value = %v,
                "INFERD_FORCE_BACKEND value not recognized; falling back to auto-detect"
            );
            None
        }
    }
}

/// Walk every registered ggml backend and collect the [`AcceleratorKind`]
/// it maps to. Names that don't map (BLAS, RPC, SYCL, etc.) are
/// silently dropped — they aren't candidates for the inference cascade.
fn enumerate_registered_backends() -> Vec<AcceleratorKind> {
    // SAFETY: FFI; ggml_backend_reg_count is safe to call after
    // ggml_backend_load_all (which we just did).
    let count = unsafe { ffi::ggml_backend_reg_count() };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        // SAFETY: FFI; index is bounded by reg_count; reg pointer is
        // owned by the registry and outlives this call.
        let reg = unsafe { ffi::ggml_backend_reg_get(i) };
        if reg.is_null() {
            continue;
        }
        // SAFETY: FFI contract — reg_get_name returns a C string with
        // 'static lifetime baked into the backend module's text
        // segment.
        let name_ptr = unsafe { ffi::ggml_backend_reg_name(reg) };
        if name_ptr.is_null() {
            continue;
        }
        // SAFETY: FFI contract — pointer is a NUL-terminated string.
        let name = match unsafe { CStr::from_ptr(name_ptr) }.to_str() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(kind) = name_to_kind(name) {
            out.push(kind);
        }
    }
    out
}

/// Probe per-device detail for the chosen accelerator.
///
/// Walks the ggml device list (`ggml_backend_dev_count` /
/// `ggml_backend_dev_get`) and returns the first device whose owning
/// backend registration name maps to `kind` per [`name_to_kind`].
/// Reads `ggml_backend_dev_name` for the human-readable label and
/// `ggml_backend_dev_memory(dev, &free, &total)` for VRAM total.
///
/// Returns an empty [`DeviceDetails`] for `Cpu` (the registry's CPU
/// "device" reports system RAM as `total`, which would mislead the
/// admin status surface) and for any kind with no matching device.
/// Caller should set `device_name` / `vram_total_bytes` to `None`
/// when fields are empty.
pub(super) fn probe_device_for_kind(kind: AcceleratorKind) -> DeviceDetails {
    if kind == AcceleratorKind::Cpu {
        // ggml's CPU device reports host RAM as total. That's not a
        // useful "VRAM" answer and would lie on the admin surface, so
        // suppress it explicitly. The `kind == Cpu` branch only fires
        // when the cascade fell through to CPU anyway, in which case
        // there's nothing accelerator-shaped to report.
        return DeviceDetails::default();
    }
    // SAFETY: FFI; safe to call after `ggml_backend_load_all`, which
    // probe_accelerator_uncached() ran before this function.
    let count = unsafe { ffi::ggml_backend_dev_count() };
    for i in 0..count {
        // SAFETY: FFI; index bounded by dev_count; pointer owned by
        // the registry and outlives this call.
        let dev = unsafe { ffi::ggml_backend_dev_get(i) };
        if dev.is_null() {
            continue;
        }
        // SAFETY: FFI; reg pointer is owned by the registry.
        let reg = unsafe { ffi::ggml_backend_dev_backend_reg(dev) };
        if reg.is_null() {
            continue;
        }
        // SAFETY: FFI; reg_name returns a 'static C string for the
        // lifetime of the loaded module.
        let reg_name_ptr = unsafe { ffi::ggml_backend_reg_name(reg) };
        if reg_name_ptr.is_null() {
            continue;
        }
        // SAFETY: FFI contract — pointer is a NUL-terminated string.
        let reg_name = match unsafe { CStr::from_ptr(reg_name_ptr) }.to_str() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if name_to_kind(reg_name) != Some(kind) {
            continue;
        }

        // Matching device. Read name + VRAM. Either may be missing on
        // a given backend; treat null/empty as None.
        let name = read_dev_name(dev);
        let total_bytes = read_dev_total_memory(dev);
        return DeviceDetails { name, total_bytes };
    }
    DeviceDetails::default()
}

/// Read `ggml_backend_dev_name`, returning `None` for null / empty /
/// non-UTF-8 results. Empty strings are dropped because they convey no
/// information and would round-trip as `""` on the admin surface.
fn read_dev_name(dev: *mut ffi::ggml_backend_device) -> Option<String> {
    // SAFETY: FFI; dev validated by caller.
    let ptr = unsafe { ffi::ggml_backend_dev_name(dev) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: FFI contract — NUL-terminated string with module-static
    // lifetime.
    let s = unsafe { CStr::from_ptr(ptr) }.to_str().ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Read `ggml_backend_dev_memory(dev, &free, &total)` and return the
/// `total` value. Drops the free value — it changes second-to-second
/// and reporting it would force the admin surface to either lie or
/// re-probe on every emit (see [`crate::backend::AcceleratorInfo`]).
/// Returns `None` if the backend reports zero (some backends use 0 to
/// mean "unknown").
fn read_dev_total_memory(dev: *mut ffi::ggml_backend_device) -> Option<u64> {
    let mut free: usize = 0;
    let mut total: usize = 0;
    // SAFETY: FFI; dev validated by caller; both out pointers are
    // local stack slots valid for the call.
    unsafe { ffi::ggml_backend_dev_memory(dev, &mut free, &mut total) };
    if total == 0 { None } else { Some(total as u64) }
}

/// Map a `ggml_backend_reg_name` string to the matching
/// [`AcceleratorKind`].
///
/// Reg names are stable upstream literals:
/// - `"CPU"` — `ggml-cpu/ggml-cpu.cpp`
/// - `"MTL"` — `ggml-metal/ggml-metal.cpp` (alias for Metal)
/// - `"CUDA"` — `ggml-cuda/ggml-cuda.cu` (NVIDIA build)
/// - `"ROCm"` — same source, but `GGML_CUDA_NAME` flips to `"ROCm"`
///   when `GGML_USE_HIP` is set
/// - `"Vulkan"` — `ggml-vulkan/ggml-vulkan.cpp`
///
/// Other registered backends (`"BLAS"`, `"RPC"`, `"SYCL"`, `"CANN"`,
/// etc.) aren't candidates for the cascade — they're either auxiliary
/// (BLAS), vendor-specific accelerators we don't ship with v0.3, or
/// network adapters. They're filtered out at the call site.
fn name_to_kind(name: &str) -> Option<AcceleratorKind> {
    match name {
        "CPU" => Some(AcceleratorKind::Cpu),
        "MTL" | "Metal" => Some(AcceleratorKind::Metal),
        "CUDA" => Some(AcceleratorKind::Cuda),
        "ROCm" | "HIP" => Some(AcceleratorKind::Rocm),
        "Vulkan" => Some(AcceleratorKind::Vulkan),
        _ => None,
    }
}

/// ADR 0019 cascade: pick the strongest available, fall through to CPU.
///
/// `Metal > CUDA > ROCm > Vulkan > CPU`. NPU paths (OpenVINO / ANE /
/// DirectML-NPU / QNN) are deliberately excluded — LLM decode lags
/// CPU+SIMD on every shipping NPU in 2026.
fn pick_from_cascade(registered: &[AcceleratorKind]) -> AcceleratorKind {
    const CASCADE: [AcceleratorKind; 4] = [
        AcceleratorKind::Metal,
        AcceleratorKind::Cuda,
        AcceleratorKind::Rocm,
        AcceleratorKind::Vulkan,
    ];
    for candidate in CASCADE {
        if registered.contains(&candidate) {
            return candidate;
        }
    }
    AcceleratorKind::Cpu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_prefers_metal() {
        let registered = [
            AcceleratorKind::Cpu,
            AcceleratorKind::Vulkan,
            AcceleratorKind::Metal,
        ];
        assert_eq!(pick_from_cascade(&registered), AcceleratorKind::Metal);
    }

    #[test]
    fn cascade_prefers_cuda_over_rocm_and_vulkan() {
        let registered = [
            AcceleratorKind::Cpu,
            AcceleratorKind::Vulkan,
            AcceleratorKind::Rocm,
            AcceleratorKind::Cuda,
        ];
        assert_eq!(pick_from_cascade(&registered), AcceleratorKind::Cuda);
    }

    #[test]
    fn cascade_prefers_rocm_over_vulkan() {
        let registered = [
            AcceleratorKind::Cpu,
            AcceleratorKind::Vulkan,
            AcceleratorKind::Rocm,
        ];
        assert_eq!(pick_from_cascade(&registered), AcceleratorKind::Rocm);
    }

    #[test]
    fn cascade_falls_through_to_cpu() {
        let registered = [AcceleratorKind::Cpu];
        assert_eq!(pick_from_cascade(&registered), AcceleratorKind::Cpu);
    }

    #[test]
    fn cascade_with_no_registered_backends_is_cpu() {
        // Defensive: a registry that somehow returns nothing should
        // still produce a meaningful answer rather than a panic.
        assert_eq!(pick_from_cascade(&[]), AcceleratorKind::Cpu);
    }

    #[test]
    fn name_to_kind_recognises_cpu_metal_cuda_rocm_vulkan() {
        assert_eq!(name_to_kind("CPU"), Some(AcceleratorKind::Cpu));
        assert_eq!(name_to_kind("MTL"), Some(AcceleratorKind::Metal));
        assert_eq!(name_to_kind("Metal"), Some(AcceleratorKind::Metal));
        assert_eq!(name_to_kind("CUDA"), Some(AcceleratorKind::Cuda));
        assert_eq!(name_to_kind("ROCm"), Some(AcceleratorKind::Rocm));
        assert_eq!(name_to_kind("HIP"), Some(AcceleratorKind::Rocm));
        assert_eq!(name_to_kind("Vulkan"), Some(AcceleratorKind::Vulkan));
    }

    #[test]
    fn name_to_kind_drops_unknown_backends() {
        // Auxiliary / out-of-scope backends should be filtered, not
        // mapped to a misleading variant.
        assert_eq!(name_to_kind("BLAS"), None);
        assert_eq!(name_to_kind("RPC"), None);
        assert_eq!(name_to_kind("SYCL"), None);
        assert_eq!(name_to_kind("CANN"), None);
        assert_eq!(name_to_kind("OpenCL"), None);
        assert_eq!(name_to_kind(""), None);
    }
}
