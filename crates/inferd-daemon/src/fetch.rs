//! First-boot model bootstrap into the shared CAS store.
//!
//! Per ADR 0010 the daemon may issue outbound HTTPS for one purpose
//! only: fetching a pinned GGUF named in `~/.inferd/config.json`. Per
//! ADR 0011 the bytes land in the shared content-addressable store
//! at `$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`, with a manifest
//! written at `$MODELS_HOME/manifests/<name>.json`.
//!
//! Producer flow:
//!
//! 1. Acquire `LOCK_EX` on `$MODELS_HOME/locks/<name>.lock`.
//! 2. If the manifest already names a blob and that blob exists:
//!    optional re-verify, then return the blob path immediately.
//! 3. Otherwise stream HTTPS into
//!    `$MODELS_HOME/blobs/sha256/<aa>/.partial-<hash>/data.tmp`
//!    with a running SHA-256.
//! 4. Constant-time compare computed vs expected SHA (F-5).
//! 5. On match: atomic-rename into place, write manifest. On
//!    mismatch: move bad bytes into `locks/quarantine/` and bail.
//! 6. Release the lock.
//!
//! Progress events publish through a `StatusBroadcaster` so the
//! admin socket can fan them out to UIs and middleware.

use crate::admin::StatusBroadcaster;
use crate::status::{LoadPhase, StatusEvent};
use crate::store::{Manifest, ManifestSource, ModelStore, format_blob_ref, parse_blob_ref};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

/// One downloadable GGUF model. The fetch contract is one URL +
/// expected SHA-256; anything more elaborate (registries, mirrors)
/// belongs in the operator's HTTP proxy or a `wget` step, not here.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// Stable identifier, e.g. `"gemma-4-e4b"`. Used as the manifest
    /// filename (`<name>.json`) and the lock-file basename.
    pub name: String,
    /// Direct-download HTTPS endpoint. Must be `https://`. Empty
    /// string is permitted for CLI-only mode where the operator has
    /// pre-placed bytes at a manifest-defined blob path.
    pub source_url: String,
    /// Lowercase hex SHA-256 of the GGUF bytes. Required.
    pub sha256_hex: String,
    /// Advisory total size for progress reporting + manifest. `None`
    /// = unknown (Content-Length missing); progress frames omit
    /// `total_bytes` and the manifest records the actually-downloaded
    /// size.
    pub size_bytes: Option<u64>,
    /// SPDX-style license id when known. Recorded in the manifest
    /// for cross-tool consumers; not consulted at runtime.
    pub license: Option<String>,
    /// Diagnostic provenance for the manifest. Optional — falls back
    /// to a derived shape from `source_url` if absent.
    pub source: Option<ManifestSource>,
}

/// Errors produced by `fetch_model`.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// `source_url` was not `https://`.
    #[error("model URL must be https:// (got {0:?})")]
    InsecureUrl(String),
    /// HTTP transport error (DNS, TLS, refused connection).
    #[error("http transport: {0}")]
    Transport(String),
    /// Server returned a non-success status.
    #[error("http status {0}")]
    HttpStatus(u16),
    /// I/O error reading body or writing dest.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// SHA-256 mismatch between downloaded bytes and `sha256_hex`.
    /// File has been moved into `locks/quarantine/`.
    #[error(
        "SHA-256 mismatch (expected {expected}, got {actual}); quarantined to {quarantine_path}"
    )]
    HashMismatch {
        /// What the config said.
        expected: String,
        /// What we computed.
        actual: String,
        /// Where the bad bytes were moved.
        quarantine_path: PathBuf,
    },
    /// Atomic rename of the partial into the final blob path failed.
    #[error("finalise rename: {0}")]
    Finalise(io::Error),
    /// Another producer holds the per-name lock. Another daemon is
    /// currently fetching this same model.
    #[error("model {name:?} is being fetched by another process")]
    LockContended {
        /// Model name that was contended.
        name: String,
    },
    /// CLI-only mode: source_url is empty AND no manifest exists.
    /// Operator must either set source_url or pre-write a manifest +
    /// blob.
    #[error("model {name:?} has no source_url and no manifest exists")]
    NoSourceNoManifest {
        /// Model name that couldn't be resolved.
        name: String,
    },
}

/// Resolve a model into its CAS blob path.
///
/// If the manifest exists and its referenced blob is on disk, return
/// the blob path immediately (no network, no re-hash by default).
/// Otherwise — if `source_url` is set — download into the partial
/// area, verify, atomic-rename into place, write the manifest, and
/// return the blob path.
///
/// Publishes phase events through `broadcaster`:
/// - `CheckingLocal { path }` on entry.
/// - `Download { downloaded, total, source_url }` periodically.
/// - `Verify { path }` after download completes.
/// - `Quarantine { ... }` on SHA mismatch.
pub fn fetch_model(
    spec: &ModelSpec,
    store: &ModelStore,
    broadcaster: &StatusBroadcaster,
) -> Result<PathBuf, FetchError> {
    store.ensure_layout()?;

    let blob_path = store.blob_path(&spec.sha256_hex);

    // Phase 1: check the manifest + blob.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal {
            path: blob_path.clone(),
        },
    });

    if let Some(manifest) = store.read_manifest(&spec.name)? {
        // Manifest names a SHA. If it matches the expected SHA AND
        // the blob is on disk, we're done — content addressing is
        // the trust boundary.
        if let Some(manifest_sha) = parse_blob_ref(&manifest.blob) {
            if hex_ct_eq(manifest_sha, &spec.sha256_hex) && blob_path.exists() {
                info!(
                    name = %spec.name,
                    blob = %blob_path.display(),
                    "manifest + blob already present; skipping fetch"
                );
                return Ok(blob_path);
            }
            if !hex_ct_eq(manifest_sha, &spec.sha256_hex) {
                warn!(
                    name = %spec.name,
                    expected = %spec.sha256_hex,
                    in_manifest = %manifest_sha,
                    "manifest blob ref disagrees with config sha; rewriting manifest"
                );
            }
        }
    }

    // Acquire the per-name lock. Held until the function returns.
    let _lock = acquire_name_lock(store, &spec.name)?;

    // Re-check after lock acquisition (someone else may have
    // finished between phase 1 and the lock).
    if blob_path.exists() {
        let actual = sha256_of_path(&blob_path)?;
        if hex_ct_eq(&actual, &spec.sha256_hex) {
            // Make sure the manifest reflects current truth.
            write_manifest_for(store, spec, blob_path.metadata()?.len())?;
            info!(name = %spec.name, "blob landed by concurrent producer; manifest written");
            return Ok(blob_path);
        }
        // Blob exists at the right path but bytes are wrong. The CAS
        // path IS the hash, so this should be impossible without
        // tampering. Quarantine and re-fetch.
        warn!(
            name = %spec.name,
            expected = %spec.sha256_hex,
            actual = %actual,
            "blob at CAS path failed re-hash; quarantining"
        );
        let qpath = store.quarantine(&blob_path, "sha-mismatch")?;
        broadcaster.publish(StatusEvent::LoadingModel {
            phase: LoadPhase::Quarantine {
                path: blob_path.clone(),
                expected_sha256: spec.sha256_hex.clone(),
                actual_sha256: actual,
                quarantine_path: qpath,
            },
        });
    }

    // Phase 2: download — guarded by source_url presence.
    if spec.source_url.is_empty() {
        return Err(FetchError::NoSourceNoManifest {
            name: spec.name.clone(),
        });
    }
    if !spec.source_url.starts_with("https://") {
        return Err(FetchError::InsecureUrl(spec.source_url.clone()));
    }

    let partial = store.partial_path(&spec.sha256_hex);
    if let Some(parent) = partial.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let downloaded = download_with_progress(spec, &partial, broadcaster)?;

    // Phase 3: verify.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Verify {
            path: partial.clone(),
        },
    });
    let actual = sha256_of_path(&partial)?;
    if !hex_ct_eq(&actual, &spec.sha256_hex) {
        let qpath = store.quarantine(&partial, "sha-mismatch")?;
        broadcaster.publish(StatusEvent::LoadingModel {
            phase: LoadPhase::Quarantine {
                path: partial.clone(),
                expected_sha256: spec.sha256_hex.clone(),
                actual_sha256: actual.clone(),
                quarantine_path: qpath.clone(),
            },
        });
        // Best-effort cleanup of the empty `.partial-<hash>` dir.
        if let Some(parent) = partial.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        return Err(FetchError::HashMismatch {
            expected: spec.sha256_hex.clone(),
            actual,
            quarantine_path: qpath,
        });
    }

    // Phase 4: atomic rename into the CAS path.
    if let Some(parent) = blob_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&partial, &blob_path).map_err(FetchError::Finalise)?;
    if let Some(parent) = partial.parent() {
        let _ = std::fs::remove_dir(parent);
    }

    // Phase 5: write manifest last. Readers don't trust a manifest
    // until its blob is on disk, so manifest-after-blob is the safe
    // ordering.
    write_manifest_for(store, spec, downloaded)?;
    info!(
        name = %spec.name,
        blob = %blob_path.display(),
        "model installed"
    );
    Ok(blob_path)
}

/// RAII handle on `$MODELS_HOME/locks/<name>.lock`. Dropped at
/// function exit releases the lock.
struct NameLock {
    _file: File,
}

fn acquire_name_lock(store: &ModelStore, name: &str) -> Result<NameLock, FetchError> {
    let lock_path = store.lock_path(name);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(NameLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(FetchError::LockContended {
            name: name.to_string(),
        }),
        Err(TryLockError::Error(e)) => Err(FetchError::Io(e)),
    }
}

fn write_manifest_for(
    store: &ModelStore,
    spec: &ModelSpec,
    size_bytes: u64,
) -> Result<(), FetchError> {
    let source = spec.source.clone().unwrap_or_else(|| ManifestSource {
        registry: registry_from_url(&spec.source_url),
        repo: String::new(),
        revision: String::new(),
        filename: filename_from_url(&spec.source_url),
    });
    let manifest = Manifest {
        schema_version: 1,
        name: spec.name.clone(),
        format: "gguf".into(),
        blob: format_blob_ref(&spec.sha256_hex),
        size_bytes,
        license: spec.license.clone(),
        source,
        produced_by: format!("inferd/{}", env!("CARGO_PKG_VERSION")),
        produced_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    store
        .write_manifest(&manifest)
        .map_err(FetchError::Io)
        .map(|_| ())
}

fn registry_from_url(url: &str) -> String {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_string()
}

fn filename_from_url(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("").to_string()
}

fn download_with_progress(
    spec: &ModelSpec,
    dest: &Path,
    broadcaster: &StatusBroadcaster,
) -> Result<u64, FetchError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .build();

    info!(
        url = %spec.source_url,
        name = %spec.name,
        "model download starting"
    );

    let resp = agent
        .get(&spec.source_url)
        .call()
        .map_err(|e| FetchError::Transport(e.to_string()))?;
    let status = resp.status();
    if !(200..300).contains(&status) {
        return Err(FetchError::HttpStatus(status));
    }
    let total = resp
        .header("content-length")
        .and_then(|s| s.parse::<u64>().ok())
        .or(spec.size_bytes);
    if let Some(t) = total {
        info!(
            total_bytes = t,
            total_mib = t / (1024 * 1024),
            "model download size known"
        );
    } else {
        info!("model download size unknown (no Content-Length)");
    }

    let mut reader = resp.into_reader();
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)?;

    let mut buf = vec![0u8; 1 << 20]; // 1 MiB
    let mut downloaded: u64 = 0;
    let mut last_publish = Instant::now();
    let mut next_byte_milestone: u64 = 32 << 20; // every 32 MiB

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Download {
            downloaded_bytes: 0,
            total_bytes: total,
            source_url: spec.source_url.clone(),
        },
    });

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        let now = Instant::now();
        let due = downloaded >= next_byte_milestone
            || now.duration_since(last_publish) >= Duration::from_secs(5);
        if due {
            broadcaster.publish(StatusEvent::LoadingModel {
                phase: LoadPhase::Download {
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                    source_url: spec.source_url.clone(),
                },
            });
            // Stdout/journal-visible progress so an operator running
            // the daemon manually (or watching journalctl) sees the
            // download is alive. Without this the daemon was silent
            // for the duration of a 5 GB pull. Mirrors the milestone
            // cadence of the admin-socket event so subscribers and
            // log tailers see the same numbers.
            let pct = total
                .map(|t| (downloaded as f64 / t as f64) * 100.0)
                .map(|p| format!("{p:5.1}%"))
                .unwrap_or_else(|| "  ?  ".to_string());
            let mib = downloaded / (1024 * 1024);
            let total_mib = total.map(|t| t / (1024 * 1024)).unwrap_or(0);
            info!(
                downloaded_mib = mib,
                total_mib = total_mib,
                pct = %pct,
                "model download progress"
            );
            last_publish = now;
            next_byte_milestone = downloaded + (32 << 20);
        }
    }
    file.flush()?;

    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Download {
            downloaded_bytes: downloaded,
            total_bytes: total.or(Some(downloaded)),
            source_url: spec.source_url.clone(),
        },
    });
    info!(
        downloaded_mib = downloaded / (1024 * 1024),
        "model download complete"
    );
    Ok(downloaded)
}

/// Streaming SHA-256 of a file as lowercase hex.
fn sha256_of_path(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    Ok(s)
}

fn hex_ct_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // SHA-256("hello world").
    const HELLO_WORLD_SHA: &str =
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    fn dummy_broadcaster() -> StatusBroadcaster {
        StatusBroadcaster::new(StatusEvent::Starting)
    }

    fn write_blob_at(store: &ModelStore, sha: &str, contents: &[u8]) -> PathBuf {
        let blob = store.blob_path(sha);
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::write(&blob, contents).unwrap();
        blob
    }

    #[test]
    fn fetch_returns_immediately_when_manifest_and_blob_present() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        store.ensure_layout().unwrap();

        // Pre-seed manifest + blob.
        let blob = write_blob_at(&store, HELLO_WORLD_SHA, b"hello world");
        let manifest = Manifest {
            schema_version: 1,
            name: "test".into(),
            format: "gguf".into(),
            blob: format_blob_ref(HELLO_WORLD_SHA),
            size_bytes: 11,
            license: None,
            source: ManifestSource {
                registry: "example.invalid".into(),
                repo: String::new(),
                revision: String::new(),
                filename: "blob.gguf".into(),
            },
            produced_by: "test".into(),
            produced_at: "2026-05-18T00:00:00Z".into(),
        };
        store.write_manifest(&manifest).unwrap();

        let spec = ModelSpec {
            name: "test".into(),
            source_url: "https://example.invalid/blob.gguf".into(),
            sha256_hex: HELLO_WORLD_SHA.into(),
            size_bytes: Some(11),
            license: None,
            source: None,
        };

        let b = dummy_broadcaster();
        let mut rx = b.subscribe();
        let got = fetch_model(&spec, &store, &b).unwrap();
        assert_eq!(got, blob);

        let ev = rx.try_recv().unwrap();
        assert!(matches!(
            ev,
            StatusEvent::LoadingModel {
                phase: LoadPhase::CheckingLocal { .. }
            }
        ));
    }

    #[test]
    fn fetch_quarantines_blob_with_wrong_bytes() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        store.ensure_layout().unwrap();

        // Place WRONG bytes at the CAS path for HELLO_WORLD_SHA.
        let blob = write_blob_at(&store, HELLO_WORLD_SHA, b"different bytes");

        let spec = ModelSpec {
            name: "test".into(),
            source_url: "https://example.invalid/blob.gguf".into(),
            sha256_hex: HELLO_WORLD_SHA.into(),
            size_bytes: Some(11),
            license: None,
            source: None,
        };
        let b = dummy_broadcaster();
        // Will fail to reach example.invalid AFTER quarantining the
        // bad blob, which is the path under test.
        let _ = fetch_model(&spec, &store, &b);

        assert!(!blob.exists(), "bad blob should have been quarantined");
        let qdir = store.quarantine_dir();
        assert!(qdir.is_dir());
        let entries: Vec<_> = std::fs::read_dir(&qdir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            !entries.is_empty(),
            "expected at least one quarantined file"
        );
    }

    #[test]
    fn fetch_rejects_non_https_url() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        let spec = ModelSpec {
            name: "test".into(),
            source_url: "http://example.invalid/blob.gguf".into(),
            sha256_hex: HELLO_WORLD_SHA.into(),
            size_bytes: None,
            license: None,
            source: None,
        };
        let b = dummy_broadcaster();
        let err = fetch_model(&spec, &store, &b).unwrap_err();
        assert!(matches!(err, FetchError::InsecureUrl(_)));
    }

    #[test]
    fn fetch_errors_when_no_source_and_no_manifest() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        let spec = ModelSpec {
            name: "test".into(),
            source_url: String::new(),
            sha256_hex: HELLO_WORLD_SHA.into(),
            size_bytes: None,
            license: None,
            source: None,
        };
        let b = dummy_broadcaster();
        let err = fetch_model(&spec, &store, &b).unwrap_err();
        assert!(matches!(err, FetchError::NoSourceNoManifest { .. }));
    }

    #[test]
    fn sha256_of_known_input() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("blob");
        std::fs::write(&path, b"hello world").unwrap();
        let got = sha256_of_path(&path).unwrap();
        assert_eq!(got, HELLO_WORLD_SHA);
    }

    #[test]
    fn registry_from_url_pulls_hostname() {
        assert_eq!(
            registry_from_url("https://huggingface.co/foo/bar.gguf"),
            "huggingface.co"
        );
        assert_eq!(registry_from_url("not-a-url"), "");
    }

    #[test]
    fn filename_from_url_pulls_basename() {
        assert_eq!(
            filename_from_url("https://huggingface.co/foo/x.gguf"),
            "x.gguf"
        );
    }
}
