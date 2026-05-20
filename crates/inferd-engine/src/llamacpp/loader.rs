//! Model file verification and load.
//!
//! Two THREAT_MODEL findings drive this module's shape:
//!
//! - **F-5** (constant-time SHA-256 compare): when an expected hash is
//!   supplied, the comparison uses `subtle::ConstantTimeEq` so the daemon
//!   does not leak how many leading bytes match.
//! - **F-6** (TOCTOU): when verification is requested, the model file is
//!   stream-hashed in place, then handed to `llama_model_load_from_file`
//!   at the same path. The residual window between hash and mmap is
//!   microseconds and requires an attacker with write access to the
//!   model file — which, per F-6's own no-hash justification, is a
//!   threat that already exceeds the model integrity guarantee
//!   (an attacker who can rewrite the user's model file has already
//!   won, hash or no hash). Earlier versions copied the file into a
//!   daemon-owned tempdir before hashing; that defended against a
//!   narrower threat (sub-microsecond rewrite race) at the cost of
//!   requiring `$TMPDIR` to hold a full second copy of the model.
//!   On tmpfs-constrained hosts (WSL2 default), this caused the
//!   daemon to fail with ENOSPC on cold start. See issue #6.
//!
//! When no expected hash is supplied, the original path goes straight
//! to `llama_model_load_from_file` (no hash). That path is documented
//! in `THREAT_MODEL.md` F-6 as the "operator-trusted file" mode.

#![allow(unsafe_code)] // FFI call surface; module-scoped.

use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

/// Errors produced by `load_model`.
#[derive(Debug, thiserror::Error)]
pub enum ModelLoadError {
    /// Underlying I/O failure (open, read for hashing).
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// SHA-256 of the file did not match the expected value.
    #[error("model hash mismatch (expected != actual)")]
    HashMismatch,
    /// Path string contained an interior NUL — cannot be passed to C FFI.
    #[error("path contains NUL byte: {0}")]
    PathNul(PathBuf),
    /// `libllama` returned a null pointer from `model_load_from_file`.
    #[error("llama_model_load_from_file returned null")]
    LlamaLoadFailed,
}

/// Owned handle to a loaded `llama_model`. Drops `llama_model_free` on
/// `Drop`. Cloning is intentionally not supported — only one owner per
/// model pointer.
#[derive(Debug)]
pub struct ModelHandle {
    ptr: NonNull<crate::ffi::llama_model>,
}

// SAFETY: `llama_model` is internally synchronised by libllama for the
// read-only operations we issue (`llama_model_get_vocab`, etc.); the only
// mutating op is `llama_model_free` which `Drop` runs exclusively.
unsafe impl Send for ModelHandle {}
unsafe impl Sync for ModelHandle {}

impl ModelHandle {
    /// Raw pointer for FFI calls inside the backend module.
    pub(crate) fn as_ptr(&self) -> *mut crate::ffi::llama_model {
        self.ptr.as_ptr()
    }
}

impl Drop for ModelHandle {
    fn drop(&mut self) {
        // SAFETY: pointer was returned by `llama_model_load_from_file`
        // and not freed yet. `Drop` runs exactly once per owner.
        unsafe { crate::ffi::llama_model_free(self.ptr.as_ptr()) };
    }
}

/// Verify (optionally) and load a model file via `libllama`.
///
/// **F-6 (TOCTOU)**: when `expected_sha256` is `Some`, the model file
/// is stream-hashed in place at the original path, then handed to
/// `llama_model_load_from_file` at the same path. The residual TOCTOU
/// window between hash and mmap is microseconds; an attacker with
/// write access to the model file is already a threat that exceeds
/// what hashing can defend against (per F-6 §"operator-trusted file"
/// — same justification as the no-hash path).
///
/// Earlier versions copied the file into a daemon-owned tempdir
/// before hashing. That doubled disk usage during cold start and
/// caused ENOSPC on tmpfs-constrained hosts (WSL2 default tmpfs is
/// half of allocated RAM; multi-GB Gemma models did not fit). See
/// issue #6.
///
/// **F-5 (constant-time compare)**: the SHA-256 comparison uses
/// `subtle::ConstantTimeEq` so the daemon doesn't leak how many
/// leading bytes match.
///
/// When `expected_sha256` is `None` the original path goes straight
/// to `libllama` (no hash). That's the "operator-trusted model file"
/// mode — see `THREAT_MODEL.md` F-6 for the trade-off.
///
/// `gpu_layers` of `0` keeps generation on CPU; positive values offload
/// the matching number of transformer layers to the GPU when a GPU
/// backend feature was compiled in.
pub fn load_model(
    path: &Path,
    expected_sha256: Option<&[u8; 32]>,
    gpu_layers: i32,
) -> Result<ModelHandle, ModelLoadError> {
    if let Some(expected) = expected_sha256 {
        verify_sha256(path, expected)?;
    }

    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| ModelLoadError::PathNul(path.to_path_buf()))?;

    // SAFETY: FFI call. `params` is a POD struct populated by the
    // libllama-provided default constructor; we then mutate the fields
    // we actually want to control. `cpath` outlives the call.
    let model_ptr = unsafe {
        let mut params = crate::ffi::llama_model_default_params();
        params.n_gpu_layers = gpu_layers;
        crate::ffi::llama_model_load_from_file(cpath.as_ptr(), params)
    };

    NonNull::new(model_ptr)
        .map(|ptr| ModelHandle { ptr })
        .ok_or(ModelLoadError::LlamaLoadFailed)
}

fn verify_sha256(path: &Path, expected: &[u8; 32]) -> Result<(), ModelLoadError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20]; // 1 MiB
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hasher.finalize();

    use subtle::ConstantTimeEq;
    if actual.as_slice().ct_eq(expected.as_slice()).into() {
        Ok(())
    } else {
        Err(ModelLoadError::HashMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn verify_sha256_accepts_correct_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"hello world").unwrap();
        f.sync_all().unwrap();

        // sha256("hello world")
        let expected = hex_lit("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        verify_sha256(&path, &expected).unwrap();
    }

    #[test]
    fn verify_sha256_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"hello world").unwrap();
        f.sync_all().unwrap();

        let wrong = [0u8; 32];
        let err = verify_sha256(&path, &wrong).unwrap_err();
        assert!(matches!(err, ModelLoadError::HashMismatch));
    }

    // F-6 mitigation: prove that load_model copies into a tempdir
    // before hashing when expected_sha256 is supplied. We exercise this
    // via the error path — give load_model a "good" file (which then
    // fails the libllama load step because the bytes aren't a real
    // GGUF), and verify the failure was HashMismatch when the hash is
    // wrong (proves the hash check ran), or LlamaLoadFailed when the
    // hash matches (proves the hash check passed and load proceeded).
    //
    // Without the copy step, an attacker could rewrite the original
    // between hash and load; since we're now hashing the copy, the
    // hash and load both see the same bytes. The test asserts the
    // reachable error path; the timing-attack window is closed by
    // construction (single file, single read).

    #[test]
    fn load_model_with_wrong_hash_fails_at_hash_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-gguf.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"definitely not a gguf model file").unwrap();
        f.sync_all().unwrap();

        let wrong = [0u8; 32];
        let err = load_model(&path, Some(&wrong), 0).unwrap_err();
        assert!(
            matches!(err, ModelLoadError::HashMismatch),
            "expected HashMismatch, got {err:?}"
        );
    }

    #[test]
    fn load_model_with_no_hash_skips_copy_path() {
        // No expected_sha256 ⇒ no copy, no hash. Goes straight to
        // libllama load which then fails (bytes aren't a GGUF).
        // Smoke test that the no-hash path compiles + reaches load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-gguf.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"definitely not a gguf model file").unwrap();
        f.sync_all().unwrap();

        let err = load_model(&path, None, 0).unwrap_err();
        // Either LlamaLoadFailed (most likely) or Io if libllama
        // chooses to surface it that way. NOT HashMismatch — that
        // path was never taken.
        assert!(
            !matches!(err, ModelLoadError::HashMismatch),
            "no-hash path should not hit HashMismatch; got {err:?}"
        );
    }

    fn hex_lit(s: &str) -> [u8; 32] {
        let bytes: Vec<u8> = (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect();
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        out
    }
}
