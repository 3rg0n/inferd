//! Shared content-addressable model store (ADR 0011).
//!
//! Layout:
//!
//! ```text
//! $MODELS_HOME/
//! ├── blobs/
//! │   └── sha256/
//! │       └── <aa>/                          # 2-char fanout
//! │           └── <full-hash>/
//! │               └── data
//! ├── manifests/
//! │   └── <name>.json
//! └── locks/
//!     ├── <name>.lock                        # advisory write lock
//!     └── quarantine/
//!         └── <ts>-<reason>.bin              # bad blobs, kept for forensics
//! ```
//!
//! Resolution order for `$MODELS_HOME` (first hit wins):
//!
//! 1. `models_home` field in the operator config file.
//! 2. `MODELS_HOME` env var.
//! 3. Platform default:
//!    - Linux/*BSD: `${XDG_DATA_HOME:-$HOME/.local/share}/models/`
//!    - macOS:      `~/Library/Application Support/models/`
//!    - Windows:    `%LOCALAPPDATA%\models\`
//!
//! Windows MUST NOT default to `%APPDATA%` (Roaming). Roaming
//! profiles upload `%APPDATA%` to the domain controller / OneDrive,
//! which would replicate multi-GB blobs to every machine the user
//! signs into.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Default `$MODELS_HOME` for the running platform.
///
/// Resolution chain documented in the module header.
pub fn default_models_home() -> PathBuf {
    if let Some(p) = std::env::var_os("MODELS_HOME") {
        let pb = PathBuf::from(p);
        if !pb.as_os_str().is_empty() {
            return pb;
        }
    }
    platform_default()
}

#[cfg(target_os = "linux")]
fn platform_default() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let pb = PathBuf::from(xdg);
        if !pb.as_os_str().is_empty() {
            return pb.join("models");
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("models")
}

#[cfg(target_os = "macos")]
fn platform_default() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
        .join("Application Support")
        .join("models")
}

#[cfg(windows)]
fn platform_default() -> PathBuf {
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        let pb = PathBuf::from(p);
        if !pb.as_os_str().is_empty() {
            return pb.join("models");
        }
    }
    // Sensible fallback if LOCALAPPDATA is somehow unset.
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AppData")
        .join("Local")
        .join("models")
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_default() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("models")
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

/// Handle to a model store rooted at one `$MODELS_HOME`.
///
/// Cheap to construct (just resolves paths). All disk creation is
/// lazy: directories are made on first write.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    /// Open a store rooted at `root`. Resolves `~` to the home dir.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let mut root = root.into();
        if let Some(stripped) = root
            .to_str()
            .and_then(|s| s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")))
            && let Some(home) = home_dir()
        {
            root = home.join(stripped);
        }
        Self { root }
    }

    /// Open the store at the platform default location.
    pub fn at_platform_default() -> Self {
        Self::open(default_models_home())
    }

    /// Root path of this store (`$MODELS_HOME`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the `blobs/sha256/<aa>/<full-hash>/data` blob for
    /// a SHA-256 hex string. Does NOT check existence.
    pub fn blob_path(&self, sha256_hex: &str) -> PathBuf {
        let aa = sha256_hex.get(..2).unwrap_or("00");
        self.root
            .join("blobs")
            .join("sha256")
            .join(aa)
            .join(sha256_hex)
            .join("data")
    }

    /// Path to the in-progress `data.tmp` for a download. Lives in
    /// a `.partial-<hash>` sibling so abandoned downloads are
    /// trivially distinguishable from finalised blobs.
    pub fn partial_path(&self, sha256_hex: &str) -> PathBuf {
        let aa = sha256_hex.get(..2).unwrap_or("00");
        self.root
            .join("blobs")
            .join("sha256")
            .join(aa)
            .join(format!(".partial-{sha256_hex}"))
            .join("data.tmp")
    }

    /// Path to a manifest by name.
    pub fn manifest_path(&self, name: &str) -> PathBuf {
        self.root.join("manifests").join(format!("{name}.json"))
    }

    /// Path to the advisory lock file for `name`. Producers hold an
    /// exclusive lock on this file across blob-write + manifest-write
    /// to keep two daemons from racing on the same name.
    pub fn lock_path(&self, name: &str) -> PathBuf {
        self.root.join("locks").join(format!("{name}.lock"))
    }

    /// Directory where bad blobs are quarantined. Per ADR 0011 we
    /// keep these for forensic inspection rather than deleting them.
    pub fn quarantine_dir(&self) -> PathBuf {
        self.root.join("locks").join("quarantine")
    }

    /// Read a manifest by name. Returns `Ok(None)` if absent so
    /// callers can branch on present/missing without parsing IO
    /// kinds.
    pub fn read_manifest(&self, name: &str) -> io::Result<Option<Manifest>> {
        let path = self.manifest_path(name);
        match File::open(&path) {
            Ok(file) => {
                let manifest: Manifest =
                    serde_json::from_reader(BufReader::new(file)).map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("malformed manifest at {}: {e}", path.display()),
                        )
                    })?;
                Ok(Some(manifest))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Write a manifest atomically: write to `<path>.tmp`, then
    /// rename. The blob it references must already exist on disk —
    /// callers are expected to write the blob first.
    pub fn write_manifest(&self, manifest: &Manifest) -> io::Result<PathBuf> {
        let dir = self.root.join("manifests");
        std::fs::create_dir_all(&dir)?;
        let final_path = self.manifest_path(&manifest.name);
        let tmp_path = final_path.with_extension("json.tmp");
        {
            let file = File::create(&tmp_path)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer_pretty(&mut writer, manifest)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        // POSIX `rename` is atomic when target exists. Windows
        // `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` (which Rust's
        // `std::fs::rename` uses on Windows) is the closest
        // equivalent and is the conventional choice here.
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(final_path)
    }

    /// Move a bad-bytes file into the quarantine dir under a
    /// timestamped name. Returns the new path.
    pub fn quarantine(&self, src: &Path, reason: &str) -> io::Result<PathBuf> {
        let qdir = self.quarantine_dir();
        std::fs::create_dir_all(&qdir)?;
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let safe_reason: String = reason
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let dest = qdir.join(format!("{ts}-{safe_reason}.bin"));
        std::fs::rename(src, &dest)?;
        Ok(dest)
    }

    /// Ensure the on-disk skeleton (`blobs/`, `manifests/`,
    /// `locks/`) exists. Idempotent.
    pub fn ensure_layout(&self) -> io::Result<()> {
        std::fs::create_dir_all(self.root.join("blobs").join("sha256"))?;
        std::fs::create_dir_all(self.root.join("manifests"))?;
        std::fs::create_dir_all(self.root.join("locks"))?;
        Ok(())
    }
}

/// Manifest schema v1. Wire-compatible with the cross-tool
/// Shared Local Model Store proposal — other tools that adopt
/// the convention can read inferd's manifests and vice versa.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Always 1 for v1.
    pub schema_version: u32,
    /// Stable name (e.g. `"gemma-4-e4b"`). Maps 1:1 to the
    /// manifest filename.
    pub name: String,
    /// Format token: `"gguf"` for llama.cpp-family weights.
    pub format: String,
    /// `sha256:<64-hex>` reference into `blobs/`.
    pub blob: String,
    /// Size of the blob in bytes. Diagnostic, not authoritative.
    pub size_bytes: u64,
    /// SPDX-style license id when known. Diagnostic only.
    #[serde(default)]
    pub license: Option<String>,
    /// Where the blob came from. Diagnostic + future migration aid.
    pub source: ManifestSource,
    /// What wrote this manifest, in `<tool>/<version>` form.
    pub produced_by: String,
    /// RFC 3339 UTC timestamp.
    pub produced_at: String,
}

/// Diagnostic provenance metadata for a manifest. Not consulted at
/// runtime — the daemon trusts the SHA in `Manifest::blob`, not the
/// upstream registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSource {
    /// Hostname the blob was downloaded from.
    pub registry: String,
    /// Repo path on that host (e.g. `unsloth/gemma-4-E4B-it-GGUF`).
    pub repo: String,
    /// Branch / tag / commit identifier.
    pub revision: String,
    /// Filename in the upstream repo, for migration breadcrumbs.
    pub filename: String,
}

/// Parse a `sha256:<hex>` string into the bare hex.
pub fn parse_blob_ref(s: &str) -> Option<&str> {
    s.strip_prefix("sha256:")
}

/// Format a hex SHA-256 as a `sha256:<hex>` blob ref.
pub fn format_blob_ref(sha256_hex: &str) -> String {
    format!("sha256:{sha256_hex}")
}

#[cfg(test)]
#[allow(unsafe_code)] // Edition 2024 made env::set_var/remove_var unsafe.
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn blob_path_uses_two_char_fanout() {
        let store = ModelStore::open("/x");
        let p = store.blob_path("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789");
        let s = p.to_string_lossy();
        assert!(s.contains("blobs"));
        assert!(s.contains("sha256"));
        assert!(s.ends_with("data") || s.ends_with("data\\") || s.ends_with("data/"));
        // Fanout dir is "ab" (first two chars).
        let parts: Vec<_> = p.components().collect();
        assert!(
            parts
                .iter()
                .any(|c| c.as_os_str() == std::ffi::OsStr::new("ab"))
        );
    }

    #[test]
    fn partial_path_lives_in_dot_partial_sibling() {
        let store = ModelStore::open("/x");
        let p =
            store.partial_path("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789");
        let s = p.to_string_lossy();
        assert!(s.contains(".partial-abcdef"));
        assert!(s.ends_with("data.tmp"));
    }

    #[test]
    fn manifest_path_uses_name_dot_json() {
        let store = ModelStore::open("/x");
        let p = store.manifest_path("gemma-4-e4b");
        assert!(
            p.ends_with("manifests/gemma-4-e4b.json") || p.ends_with("manifests\\gemma-4-e4b.json")
        );
    }

    #[test]
    fn ensure_layout_creates_dirs() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        store.ensure_layout().unwrap();
        assert!(dir.path().join("blobs").join("sha256").is_dir());
        assert!(dir.path().join("manifests").is_dir());
        assert!(dir.path().join("locks").is_dir());
    }

    #[test]
    fn write_then_read_manifest_round_trip() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        let m = Manifest {
            schema_version: 1,
            name: "gemma-4-e4b".into(),
            format: "gguf".into(),
            blob: "sha256:30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36".into(),
            size_bytes: 5_126_304_928,
            license: Some("apache-2.0".into()),
            source: ManifestSource {
                registry: "huggingface.co".into(),
                repo: "unsloth/gemma-4-E4B-it-GGUF".into(),
                revision: "main".into(),
                filename: "gemma-4-E4B-it-UD-Q4_K_XL.gguf".into(),
            },
            produced_by: "inferd/0.1.0-alpha.0".into(),
            produced_at: "2026-05-18T17:06:10Z".into(),
        };
        store.write_manifest(&m).unwrap();
        let got = store.read_manifest("gemma-4-e4b").unwrap().unwrap();
        assert_eq!(got, m);
    }

    #[test]
    fn read_missing_manifest_returns_none() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        assert!(store.read_manifest("nope").unwrap().is_none());
    }

    #[test]
    fn quarantine_moves_file_under_quarantine_dir() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        store.ensure_layout().unwrap();
        let bad = dir.path().join("bad.bin");
        std::fs::write(&bad, b"bytes").unwrap();
        let qpath = store.quarantine(&bad, "sha-mismatch").unwrap();
        assert!(!bad.exists());
        assert!(qpath.exists());
        assert!(
            qpath
                .to_string_lossy()
                .contains(&format!("locks{}quarantine", std::path::MAIN_SEPARATOR))
        );
    }

    #[test]
    fn parse_blob_ref_strips_prefix() {
        assert_eq!(parse_blob_ref("sha256:abc"), Some("abc"));
        assert_eq!(parse_blob_ref("nope"), None);
    }

    #[test]
    fn default_models_home_honours_models_home_env() {
        // SAFETY: edition 2024 made env::set_var/remove_var unsafe
        // because environment mutation isn't thread-safe. Cargo runs
        // unit tests in a multi-threaded harness by default. This
        // test is fine if it's the only test in the process touching
        // MODELS_HOME — which it is — and we restore the saved value
        // before returning so we don't leak state to siblings.
        let saved = std::env::var_os("MODELS_HOME");
        unsafe {
            std::env::set_var("MODELS_HOME", "/tmp/inferd-test-models-home");
        }
        let p = default_models_home();
        assert_eq!(p, PathBuf::from("/tmp/inferd-test-models-home"));
        unsafe {
            if let Some(v) = saved {
                std::env::set_var("MODELS_HOME", v);
            } else {
                std::env::remove_var("MODELS_HOME");
            }
        }
    }
}
