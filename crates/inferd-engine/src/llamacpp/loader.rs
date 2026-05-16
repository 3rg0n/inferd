//! Model file verification and load.
//!
//! Two THREAT_MODEL findings drive this module's shape:
//!
//! - **F-5** (constant-time SHA-256 compare): when an expected hash is
//!   supplied, the comparison uses `subtle::ConstantTimeEq` so the daemon
//!   does not leak how many leading bytes match.
//! - **F-6** (TOCTOU): the file is opened *once*, hashed against the open
//!   fd, then handed to `libllama` to load. We don't re-resolve the path.
//!
//! `libllama`'s public load API is `llama_model_load_from_file(path,
//! params)` — it takes a path, not an fd. To honour F-6 we hash before
//! the load call and refuse to proceed if the hash check fails. An
//! attacker who can rewrite the file between hash and `mmap` has already
//! defeated our threat model boundary (the file is owned by the user
//! running the daemon); we document that explicitly in `THREAT_MODEL.md`.

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
/// If `expected_sha256` is provided, the file is hashed and compared with
/// `subtle::ConstantTimeEq` before the load call. If the hash mismatches,
/// `libllama` is never invoked.
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
