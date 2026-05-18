//! First-boot model bootstrap.
//!
//! Downloads a pinned GGUF model, verifies SHA-256 with a constant-time
//! compare, atomically renames into place, quarantines on mismatch. ADR
//! 0010 carves a narrow HTTPS exception to ADR 0006's lean-core rule
//! specifically for this purpose: outbound HTTPS to a configured
//! registry, scoped to the model named in `~/.inferd/config.json`.
//!
//! Progress events publish through a `tokio::sync::broadcast` channel
//! that the admin socket fans out to connected clients. The fetch
//! module never touches sockets directly — Step 3's `admin.rs` owns
//! that.

use crate::admin::StatusBroadcaster;
use crate::status::{LoadPhase, StatusEvent};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

/// One downloadable GGUF model. The fetch contract is a single URL +
/// expected SHA-256; anything more elaborate (registries, mirrors)
/// belongs in the operator's HTTP proxy or a `wget` step, not here.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// Stable identifier, e.g. `"gemma-4-e4b"`.
    pub name: String,
    /// Filename inside `models_dir`.
    pub filename: String,
    /// Direct-download HTTPS endpoint. Must be `https://`.
    pub source_url: String,
    /// Lowercase hex SHA-256 of the GGUF bytes. Required.
    pub sha256_hex: String,
    /// Advisory total size for progress reporting. `None` = unknown
    /// (Content-Length missing); progress frames omit `total_bytes`.
    pub size_bytes: Option<u64>,
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
    /// File has been moved to `<dest>.quarantine.<rfc3339>`.
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
    /// Atomic rename of the partial into the final path failed.
    #[error("finalise rename: {0}")]
    Finalise(io::Error),
}

/// Resolve a model: if the file already exists at the expected path
/// AND its SHA matches `spec.sha256_hex`, return immediately. Otherwise
/// download into `<models_dir>/<filename>.partial`, verify, atomic-rename.
///
/// Publishes `StatusEvent`s to `progress_tx` throughout. Subscribers
/// (the admin socket) get a stream of:
/// - `LoadingModel(CheckingLocal)` when entering the function.
/// - `LoadingModel(Download { downloaded, total })` periodically during
///   the body stream.
/// - `LoadingModel(Verify)` after download completes.
/// - `LoadingModel(Quarantine { ... })` if SHA mismatches.
///
/// Returns the absolute path of the verified model file.
pub fn fetch_model(
    spec: &ModelSpec,
    models_dir: &Path,
    broadcaster: &StatusBroadcaster,
) -> Result<PathBuf, FetchError> {
    let final_path = models_dir.join(&spec.filename);

    // Phase 1: check if the file is already correct on disk.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::CheckingLocal {
            path: final_path.clone(),
        },
    });
    if final_path.exists() {
        match sha256_of_path(&final_path) {
            Ok(actual) if hex_ct_eq(&actual, &spec.sha256_hex) => {
                info!(path = %final_path.display(), "model already present; SHA matches");
                return Ok(final_path);
            }
            Ok(actual) => {
                warn!(
                    path = %final_path.display(),
                    expected = %spec.sha256_hex,
                    actual = %actual,
                    "existing file SHA mismatch; quarantining"
                );
                let qpath = quarantine(&final_path)?;
                broadcaster.publish(StatusEvent::LoadingModel {
                    phase: LoadPhase::Quarantine {
                        path: final_path.clone(),
                        expected_sha256: spec.sha256_hex.clone(),
                        actual_sha256: actual,
                        quarantine_path: qpath,
                    },
                });
                // Fall through to download.
            }
            Err(e) => {
                warn!(path = %final_path.display(), error = %e, "couldn't hash existing file; redownloading");
            }
        }
    }

    // Validate URL scheme before any network call.
    if !spec.source_url.starts_with("https://") {
        return Err(FetchError::InsecureUrl(spec.source_url.clone()));
    }

    // Phase 2: download.
    std::fs::create_dir_all(models_dir)?;
    let partial = final_path.with_extension("partial");
    download_with_progress(spec, &partial, broadcaster)?;

    // Phase 3: verify.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Verify {
            path: partial.clone(),
        },
    });
    let actual = sha256_of_path(&partial)?;
    if !hex_ct_eq(&actual, &spec.sha256_hex) {
        let qpath = quarantine(&partial)?;
        broadcaster.publish(StatusEvent::LoadingModel {
            phase: LoadPhase::Quarantine {
                path: partial.clone(),
                expected_sha256: spec.sha256_hex.clone(),
                actual_sha256: actual.clone(),
                quarantine_path: qpath.clone(),
            },
        });
        return Err(FetchError::HashMismatch {
            expected: spec.sha256_hex.clone(),
            actual,
            quarantine_path: qpath,
        });
    }

    // Phase 4: atomic rename.
    std::fs::rename(&partial, &final_path).map_err(FetchError::Finalise)?;
    info!(path = %final_path.display(), "model installed");
    Ok(final_path)
}

fn download_with_progress(
    spec: &ModelSpec,
    dest: &Path,
    broadcaster: &StatusBroadcaster,
) -> Result<(), FetchError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .build();

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

    // Publish initial "downloading 0 bytes" so subscribers know we got
    // past `checking_local`.
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
            last_publish = now;
            next_byte_milestone = downloaded + (32 << 20);
        }
    }
    file.flush()?;

    // Final progress frame so the admin client sees a definitive
    // "download complete" before the verify phase starts.
    broadcaster.publish(StatusEvent::LoadingModel {
        phase: LoadPhase::Download {
            downloaded_bytes: downloaded,
            total_bytes: total.or(Some(downloaded)),
            source_url: spec.source_url.clone(),
        },
    });
    Ok(())
}

/// Move `path` aside to `<path>.quarantine.<rfc3339>`. Used for
/// SHA-mismatched files so we never silently re-download bytes that
/// purport to be the right size — that's a tampering signal.
fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let qpath = path.with_extension(format!("quarantine.{ts}"));
    std::fs::rename(path, &qpath)?;
    Ok(qpath)
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

    fn dummy_broadcaster() -> StatusBroadcaster {
        StatusBroadcaster::new(StatusEvent::Starting)
    }

    #[test]
    fn fetch_returns_immediately_when_file_present_and_hash_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.gguf");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"hello world").unwrap();
        f.sync_all().unwrap();

        let spec = ModelSpec {
            name: "test".into(),
            filename: "blob.gguf".into(),
            source_url: "https://example.invalid/blob.gguf".into(),
            // sha256("hello world")
            sha256_hex: "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into(),
            size_bytes: Some(11),
        };

        // Subscribe BEFORE calling fetch_model so we don't lose events.
        let b = dummy_broadcaster();
        let mut rx = b.subscribe();
        let got = fetch_model(&spec, dir.path(), &b).unwrap();
        assert_eq!(got, path);

        // First event must be CheckingLocal.
        let ev = rx.try_recv().unwrap();
        assert!(matches!(
            ev,
            StatusEvent::LoadingModel {
                phase: LoadPhase::CheckingLocal { .. }
            }
        ));
    }

    #[test]
    fn fetch_quarantines_existing_file_with_wrong_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.gguf");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"different bytes").unwrap();
        f.sync_all().unwrap();

        let spec = ModelSpec {
            name: "test".into(),
            filename: "blob.gguf".into(),
            // example.invalid is RFC 2606-reserved; resolves but never
            // connects. The download attempt fails AFTER quarantine,
            // which is the path we're testing.
            source_url: "https://example.invalid/blob.gguf".into(),
            sha256_hex: "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into(),
            size_bytes: Some(11),
        };

        let b = dummy_broadcaster();
        let _ = fetch_model(&spec, dir.path(), &b);

        // Original file is gone; a quarantine sibling exists.
        assert!(!path.exists(), "original file should have been quarantined");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        assert!(
            entries.iter().any(|n| n.contains("quarantine")),
            "expected a *.quarantine.* sibling in {entries:?}"
        );
    }

    #[test]
    fn fetch_rejects_non_https_url() {
        let dir = tempfile::tempdir().unwrap();
        let spec = ModelSpec {
            name: "test".into(),
            filename: "blob.gguf".into(),
            source_url: "http://example.invalid/blob.gguf".into(),
            sha256_hex: "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into(),
            size_bytes: None,
        };
        let b = dummy_broadcaster();
        let err = fetch_model(&spec, dir.path(), &b).unwrap_err();
        assert!(matches!(err, FetchError::InsecureUrl(_)));
    }

    #[test]
    fn sha256_of_known_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        std::fs::write(&path, b"hello world").unwrap();
        let got = sha256_of_path(&path).unwrap();
        assert_eq!(
            got,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
