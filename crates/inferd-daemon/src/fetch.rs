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
//!
//! # The airgapped build (ADR 0028)
//!
//! Steps 3–5 above live behind the default-on `model-fetch` feature.
//! Built with `--no-default-features`, `ureq` is not linked and this
//! module returns [`FetchError::FetchDisabled`] where it would have
//! dialled out.
//!
//! Note what is *not* gated: the module, and `fetch_model` itself.
//! Most of what this function does is local — resolve
//! `manifests/<name>.json` to a CAS blob path, take the per-name
//! writer lock, re-hash with a constant-time compare, quarantine on
//! mismatch. An airgapped deployment needs all of it, and cares about
//! the SHA verification more than anyone. Gating the module would
//! delete the local-resolution path along with the network one and
//! force a parallel implementation — which is how two artifacts start
//! to diverge.

use crate::admin::StatusBroadcaster;
use crate::status::{LoadPhase, StatusEvent};
use crate::store::{Manifest, ManifestSource, ModelStore, format_blob_ref, parse_blob_ref};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
#[cfg(feature = "model-fetch")]
use std::io::Write;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(feature = "model-fetch")]
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
    /// `model.name` cannot be used to build a store path. Rejected
    /// before any file is created, so a bad name leaves nothing behind.
    #[error(transparent)]
    InvalidName(#[from] crate::store::InvalidModelName),
    /// HTTP transport error (DNS, TLS, refused connection).
    #[cfg(feature = "model-fetch")]
    #[error("http transport: {0}")]
    Transport(String),
    /// Server returned a non-success status.
    #[cfg(feature = "model-fetch")]
    #[error("http status {0}")]
    HttpStatus(u16),
    /// This is an airgapped build (ADR 0028): the model is not already
    /// in the CAS store and there is no HTTPS client linked to fetch
    /// it. Import the GGUF out-of-band with `inferdctl import`.
    #[cfg(not(feature = "model-fetch"))]
    #[error(
        "model {name:?} is not in the model store and this is an airgapped build \
         (no model-fetch feature); import it with `inferdctl import --name {name} <path.gguf>`"
    )]
    FetchDisabled {
        /// Model name that could not be resolved locally.
        name: String,
    },
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

/// Errors produced by [`import_model`].
///
/// Deliberately not folded into [`FetchError`]: that enum's
/// `HashMismatch` quarantines the offending bytes, and quarantining an
/// operator's source file — which may be sitting on removable media
/// they carried in — would be hostile. Import never moves or modifies
/// the file it is given.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// I/O error reading the source file or writing the store.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// The source path is not a regular file.
    #[error("{0} is not a regular file")]
    NotAFile(PathBuf),
    /// `--expect-sha256` disagreed with the file's actual digest. The
    /// source file is left untouched and nothing was written to the
    /// store.
    #[error("SHA-256 mismatch: expected {expected}, file is {actual} (nothing imported)")]
    DigestMismatch {
        /// The out-of-band digest the operator supplied.
        expected: String,
        /// What the file actually hashes to.
        actual: String,
    },
    /// `--expect-sha256` was not 64 lowercase hex characters.
    #[error("--expect-sha256 must be 64 hex characters (got {0:?})")]
    MalformedDigest(String),
    /// The bytes that landed in the store did not match what was read
    /// from the source — a failing disk or a file mutated mid-copy.
    #[error("copy verification failed: wrote {expected} but store holds {actual}")]
    CopyCorrupted {
        /// Digest computed while reading the source.
        expected: String,
        /// Digest of the bytes actually on disk afterwards.
        actual: String,
    },
    /// Another producer holds the per-name lock — a daemon or another
    /// `inferdctl` is writing this same model name.
    #[error("model {name:?} is being written by another process")]
    LockContended {
        /// Model name that was contended.
        name: String,
    },
    /// `--name` cannot be used to build a store path.
    #[error(transparent)]
    InvalidName(#[from] crate::store::InvalidModelName),
}

/// Import a local GGUF into the CAS store under `name`.
///
/// This is how bytes get into an airgapped deployment (ADR 0028), and
/// it is present in both artifacts — a subcommand shipped only in the
/// hardened build is a subcommand nobody tests, and importing a
/// hand-downloaded GGUF is useful on a networked machine too.
///
/// The file is hashed as it is copied, so the digest describes exactly
/// the bytes that were read; the copy is then re-hashed in place to
/// catch a bad disk. `expect_sha256`, when supplied, is compared with
/// [`subtle`]'s constant-time equality (invariant #8) and a mismatch
/// aborts the import without writing a manifest or touching the source.
///
/// The content address is the *computed* digest, so an import without
/// `expect_sha256` still lands at the correct CAS path — it just has no
/// out-of-band claim to check it against.
pub fn import_model(
    src: &Path,
    name: &str,
    expect_sha256: Option<&str>,
    store: &ModelStore,
) -> Result<PathBuf, ImportError> {
    // Before anything touches the filesystem: `name` is interpolated
    // into the staging, lock, and manifest paths below.
    crate::store::validate_model_name(name)?;
    if let Some(hex) = expect_sha256
        && (hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(ImportError::MalformedDigest(hex.to_string()));
    }
    if !src.is_file() {
        return Err(ImportError::NotAFile(src.to_path_buf()));
    }
    store.ensure_layout()?;

    // Take the per-name lock before writing anything, so an import
    // cannot race a daemon fetching the same name.
    let _lock = acquire_name_lock(store, name).map_err(|e| match e {
        FetchError::LockContended { name } => ImportError::LockContended { name },
        FetchError::Io(e) => ImportError::Io(e),
        // `acquire_name_lock` returns only those two variants.
        other => ImportError::Io(io::Error::other(other.to_string())),
    })?;

    // Stage into a temp file first: the CAS path is derived from the
    // digest, and the digest is not known until the bytes have been
    // read. Staging under the store keeps the rename on one device.
    let staging = store.root().join("locks").join(format!(".import-{name}"));
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut staged = match copy_hashing(src, &staging) {
        Ok(staged) => staged,
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            return Err(e.into());
        }
    };
    let computed = staged.digest.clone();

    if let Some(expected) = expect_sha256
        && !hex_ct_eq(&computed, expected)
    {
        let _ = std::fs::remove_file(&staging);
        return Err(ImportError::DigestMismatch {
            expected: expected.to_string(),
            actual: computed,
        });
    }

    // Verify what actually landed, not just what we read — and do it
    // through the handle the copy wrote, so this cannot be answered by
    // a different file that has since taken over the staging path.
    // The length comes from the same read for the same reason: a
    // second `metadata()` call would re-resolve the path.
    let (on_disk, size_bytes) = match sha256_and_len_of_handle(&mut staged.file) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = std::fs::remove_file(&staging);
            return Err(e.into());
        }
    };
    if !hex_ct_eq(&on_disk, &computed) {
        let _ = std::fs::remove_file(&staging);
        return Err(ImportError::CopyCorrupted {
            expected: computed,
            actual: on_disk,
        });
    }

    // Verification is done, so release the handle: Windows refuses to
    // rename a file that still has one open.
    drop(staged.file);

    let blob_path = store.blob_path(&computed);
    if blob_path.exists() {
        // Idempotent re-import: the CAS path is the hash, so identical
        // bytes are already there. Drop the staged copy and refresh the
        // manifest so `name` points at it.
        let _ = std::fs::remove_file(&staging);
        info!(name, blob = %blob_path.display(), "blob already in store; writing manifest only");
    } else {
        if let Some(parent) = blob_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `blob_path.exists()` above was a fast path, not a guarantee:
        // a concurrent producer holding a *different* name's lock can
        // land the same content between that check and this rename
        // (the lock is per-name, the blob path is per-content). On
        // Windows a rename onto an existing file fails, so treat "it's
        // there now" as success rather than an error — the bytes are
        // content-addressed, so whoever won wrote the same bytes.
        if let Err(e) = std::fs::rename(&staging, &blob_path) {
            if blob_path.exists() {
                let _ = std::fs::remove_file(&staging);
                info!(
                    name,
                    blob = %blob_path.display(),
                    "blob landed by a concurrent producer; keeping theirs"
                );
            } else {
                let _ = std::fs::remove_file(&staging);
                return Err(e.into());
            }
        }
    }

    // Manifest last — readers don't trust a manifest until its blob is
    // on disk (same ordering `fetch_model` uses).
    //
    // If this write fails the blob stays, unreferenced by any manifest.
    // That is deliberate: the store is content-addressed and blobs are
    // shared, so a name whose manifest never landed cannot tell whether
    // *another* name already points at those bytes — deleting on the
    // way out would break that other model. An unreferenced blob costs
    // disk; a deleted shared blob costs someone else's working install.
    // Re-running the import is idempotent and adopts the blob.
    let manifest = Manifest {
        schema_version: 1,
        name: name.to_string(),
        format: "gguf".into(),
        blob: format_blob_ref(&computed),
        size_bytes,
        license: None,
        source: ManifestSource {
            registry: "local-import".into(),
            repo: String::new(),
            revision: String::new(),
            filename: src
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        },
        produced_by: format!("inferdctl/{}", env!("CARGO_PKG_VERSION")),
        produced_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    store.write_manifest(&manifest)?;
    info!(
        name,
        blob = %blob_path.display(),
        sha256 = %computed,
        size_bytes,
        "model imported"
    );
    Ok(blob_path)
}

/// A staged copy: the digest of the bytes written, plus the still-open
/// handle they were written through.
///
/// The handle is the point. Everything downstream of the copy —
/// re-hashing the landed bytes, reading the size for the manifest —
/// goes through this descriptor rather than re-opening `dest` by path.
/// A path resolves afresh on every call, so a `dest` that was a regular
/// file during the copy could be a symlink by the time it is re-stat'd;
/// a descriptor is bound to the file the copy actually wrote.
struct Staged {
    /// Lowercase-hex SHA-256 of the bytes written.
    digest: String,
    /// The write handle, retained for verification reads.
    file: File,
}

/// Stream `src` into `dest`, returning the lowercase-hex SHA-256 of the
/// bytes copied. One pass, so the digest describes what was actually
/// read rather than a second read that could differ.
fn copy_hashing(src: &Path, dest: &Path) -> io::Result<Staged> {
    let mut input = File::open(src)?;
    // `dest` is a staging path inside the store, and the store may be
    // group-readable or shared with other tools (ADR 0011). A symlink
    // planted there would make `truncate(true)` clobber the symlink's
    // *target* — the same class as THREAT_MODEL F-2, so the same
    // answer: stat with `symlink_metadata` (which does not follow) and
    // refuse rather than repair.
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("staging path is a symlink (refused): {}", dest.display()),
            ));
        }
        // A leftover regular file from a crashed run is fine to clear.
        Ok(_) => std::fs::remove_file(dest)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    // `create_new` closes the window between the check above and the
    // open: if anything appears at `dest` in between — including a
    // symlink — this fails with `AlreadyExists` instead of following it.
    let mut output = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        std::io::Write::write_all(&mut output, &buf[..n])?;
    }
    std::io::Write::flush(&mut output)?;
    Ok(Staged {
        digest: hex_of(hasher.finalize().as_slice()),
        file: output,
    })
}

/// Streaming SHA-256 of an already-open file, plus its length.
///
/// Rewinds first, so the caller can pass the handle it just wrote
/// through. Reading through the descriptor is what makes this a
/// verification of the bytes that landed rather than of whatever
/// currently answers to that path.
fn sha256_and_len_of_handle(file: &mut File) -> io::Result<(String, u64)> {
    use std::io::Seek;
    file.rewind()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        len += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((hex_of(hasher.finalize().as_slice()), len))
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

    // Phase 2: download — guarded by source_url presence. Both checks
    // below are local judgements about the config, so they stay
    // ungated: a missing or non-HTTPS `source_url` is the same
    // misconfiguration in either artifact, and naming it precisely
    // beats a blanket "fetch is disabled".
    if spec.source_url.is_empty() {
        return Err(FetchError::NoSourceNoManifest {
            name: spec.name.clone(),
        });
    }
    if !spec.source_url.starts_with("https://") {
        return Err(FetchError::InsecureUrl(spec.source_url.clone()));
    }

    // Airgapped build (ADR 0028): a valid URL is configured, and this
    // binary has no way to dial it. Stop here rather than pretend.
    #[cfg(not(feature = "model-fetch"))]
    {
        warn!(
            name = %spec.name,
            "model not in store and this is an airgapped build; use `inferdctl import`"
        );
        Err(FetchError::FetchDisabled {
            name: spec.name.clone(),
        })
    }

    #[cfg(feature = "model-fetch")]
    download_and_install(spec, store, broadcaster, &blob_path)
}

/// Phases 3–5: stream the URL into the partial area, verify the SHA
/// with a constant-time compare, atomic-rename into the CAS path, and
/// write the manifest last.
///
/// Split out of [`fetch_model`] so the airgapped build (ADR 0028) drops
/// exactly this — the network half — while keeping local resolution,
/// locking, and verification intact.
#[cfg(feature = "model-fetch")]
fn download_and_install(
    spec: &ModelSpec,
    store: &ModelStore,
    broadcaster: &StatusBroadcaster,
    blob_path: &Path,
) -> Result<PathBuf, FetchError> {
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
    std::fs::rename(&partial, blob_path).map_err(FetchError::Finalise)?;
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
    Ok(blob_path.to_path_buf())
}

/// RAII handle on `$MODELS_HOME/locks/<name>.lock`. Dropped at
/// function exit releases the lock.
struct NameLock {
    _file: File,
}

fn acquire_name_lock(store: &ModelStore, name: &str) -> Result<NameLock, FetchError> {
    let lock_path = store.lock_path(name)?;
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

/// Ungated: the "blob landed by a concurrent producer" path in
/// [`fetch_model`] calls this too, and that path exists in both
/// artifacts. `inferdctl import` writes its own manifest through
/// `ModelStore` rather than reaching in here.
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

/// The workspace's only `ureq` call site — and the reason ADR 0028 is
/// cheap: removing one dependency removes one function.
#[cfg(feature = "model-fetch")]
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
    Ok(hex_of(hasher.finalize().as_slice()))
}

/// Lowercase hex of a digest.
fn hex_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
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

    // --- import (ADR 0028) --------------------------------------------

    /// Writes `hello world` to a temp path and returns it.
    fn hello_world_file(dir: &Path) -> PathBuf {
        let src = dir.join("model.gguf");
        std::fs::write(&src, b"hello world").unwrap();
        src
    }

    #[test]
    fn import_lands_blob_at_content_address_and_writes_manifest() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());

        let blob = import_model(&src, "imported", None, &store).unwrap();

        assert_eq!(blob, store.blob_path(HELLO_WORLD_SHA));
        assert_eq!(std::fs::read(&blob).unwrap(), b"hello world");
        let manifest = store.read_manifest("imported").unwrap().unwrap();
        assert_eq!(parse_blob_ref(&manifest.blob), Some(HELLO_WORLD_SHA));
        assert_eq!(manifest.size_bytes, 11);
        assert_eq!(manifest.source.registry, "local-import");
        assert_eq!(manifest.source.filename, "model.gguf");
        // The operator's file is never moved or modified.
        assert!(src.is_file(), "source file must survive the import");
    }

    #[test]
    fn import_accepts_a_matching_expected_digest() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());

        let blob = import_model(&src, "m", Some(HELLO_WORLD_SHA), &store).unwrap();
        assert!(blob.is_file());
    }

    /// The point of `--expect-sha256`: an operator carrying a file in on
    /// removable media has a vendor digest and, before this, no way to
    /// check it. A mismatch must import nothing.
    #[test]
    fn import_refuses_a_mismatched_expected_digest_and_writes_nothing() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());
        let wrong = "0".repeat(64);

        let err = import_model(&src, "m", Some(&wrong), &store).unwrap_err();
        match err {
            ImportError::DigestMismatch { expected, actual } => {
                assert_eq!(expected, wrong);
                assert_eq!(actual, HELLO_WORLD_SHA);
            }
            other => panic!("expected DigestMismatch, got {other:?}"),
        }
        assert!(
            store.read_manifest("m").unwrap().is_none(),
            "a rejected import must not leave a manifest"
        );
        assert!(!store.blob_path(HELLO_WORLD_SHA).exists());
        assert!(src.is_file(), "source file must not be quarantined");
        // No staging turd left behind.
        let staging = store.root().join("locks").join(".import-m");
        assert!(!staging.exists(), "staging file should be cleaned up");
    }

    #[test]
    fn import_rejects_a_malformed_expected_digest_before_reading_the_file() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());

        for bad in ["deadbeef", &"g".repeat(64), &format!("{HELLO_WORLD_SHA}0")] {
            let err = import_model(&src, "m", Some(bad), &store).unwrap_err();
            assert!(
                matches!(err, ImportError::MalformedDigest(_)),
                "{bad:?} should be rejected as malformed, got {err:?}"
            );
        }
    }

    #[test]
    fn import_rejects_a_missing_or_non_file_source() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));

        let missing = dir.path().join("nope.gguf");
        assert!(matches!(
            import_model(&missing, "m", None, &store).unwrap_err(),
            ImportError::NotAFile(_)
        ));
        // A directory is not a regular file either.
        assert!(matches!(
            import_model(dir.path(), "m", None, &store).unwrap_err(),
            ImportError::NotAFile(_)
        ));
    }

    /// Re-importing identical bytes is a manifest refresh, not an error
    /// and not a re-copy — the CAS path *is* the hash.
    #[test]
    fn import_is_idempotent_and_can_realias_an_existing_blob() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());

        let first = import_model(&src, "one", None, &store).unwrap();
        let second = import_model(&src, "one", None, &store).unwrap();
        assert_eq!(first, second);

        // A second name pointing at the same blob is legitimate.
        let aliased = import_model(&src, "two", None, &store).unwrap();
        assert_eq!(aliased, first);
        for name in ["one", "two"] {
            let m = store.read_manifest(name).unwrap().unwrap();
            assert_eq!(parse_blob_ref(&m.blob), Some(HELLO_WORLD_SHA));
        }
    }

    /// THREAT_MODEL F-2, applied to the import staging path. The store
    /// may be shared with other tools (ADR 0011), so a symlink planted
    /// at the staging path must be refused rather than followed — the
    /// same answer `endpoint::bind_uds` gives for the socket path.
    ///
    /// Runs on both families: the staging path exists on Windows too,
    /// where symlink creation needs either Developer Mode or elevation,
    /// so the test skips if it can't plant one rather than failing for
    /// an unrelated reason.
    #[test]
    fn import_refuses_a_symlinked_staging_path_instead_of_following_it() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());
        store.ensure_layout().unwrap();

        // The file an attacker wants clobbered.
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"precious").unwrap();
        let staging = store.root().join("locks").join(".import-m");
        #[cfg(unix)]
        let planted = std::os::unix::fs::symlink(&victim, &staging).is_ok();
        #[cfg(windows)]
        let planted = std::os::windows::fs::symlink_file(&victim, &staging).is_ok();
        if !planted {
            eprintln!("skipping: cannot create symlinks in this environment");
            return;
        }

        let err = import_model(&src, "m", None, &store).unwrap_err();
        match err {
            ImportError::Io(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidInput),
            other => panic!("expected Io(InvalidInput), got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"precious",
            "the symlink target must not be written through"
        );
        assert!(
            store.read_manifest("m").unwrap().is_none(),
            "a refused import must not leave a manifest"
        );
    }

    /// The blob path is per-content but the writer lock is per-name, so
    /// two imports under different names can race to the same blob. The
    /// loser must succeed against the winner's bytes, not fail — on
    /// Windows `rename` onto an existing file is an error.
    #[test]
    fn import_treats_a_blob_that_appeared_mid_rename_as_success() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());

        // Stand in for the concurrent producer: the blob is already at
        // its content address, which is exactly the state the racing
        // import would observe after its own `exists()` fast path.
        let blob_path = store.blob_path(HELLO_WORLD_SHA);
        std::fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
        std::fs::write(&blob_path, b"hello world").unwrap();

        let got = import_model(&src, "m", None, &store).unwrap();
        assert_eq!(got, blob_path);
        assert_eq!(std::fs::read(&got).unwrap(), b"hello world");
        let m = store.read_manifest("m").unwrap().unwrap();
        assert_eq!(parse_blob_ref(&m.blob), Some(HELLO_WORLD_SHA));
        assert_eq!(m.size_bytes, 11, "size must come from the staged copy");
        let staging = store.root().join("locks").join(".import-m");
        assert!(!staging.exists(), "staging file should be cleaned up");
    }

    /// An airgapped build has no HTTPS client, so a model that is not
    /// already in the store is a clear error naming the way out, not a
    /// silent hang or a confusing transport failure.
    #[cfg(not(feature = "model-fetch"))]
    #[test]
    fn airgapped_fetch_reports_disabled_instead_of_dialling() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path());
        let spec = ModelSpec {
            name: "absent".into(),
            source_url: "https://example.invalid/blob.gguf".into(),
            sha256_hex: HELLO_WORLD_SHA.into(),
            size_bytes: None,
            license: None,
            source: None,
        };
        let err = fetch_model(&spec, &store, &dummy_broadcaster()).unwrap_err();
        assert!(
            matches!(err, FetchError::FetchDisabled { .. }),
            "expected FetchDisabled, got {err:?}"
        );
        assert!(err.to_string().contains("inferdctl import"));
    }

    /// The same model already present resolves locally in the airgapped
    /// build — proof that gating removed only the network half.
    #[cfg(not(feature = "model-fetch"))]
    #[test]
    fn airgapped_still_resolves_an_imported_model() {
        let dir = tempdir().unwrap();
        let store = ModelStore::open(dir.path().join("store"));
        let src = hello_world_file(dir.path());
        import_model(&src, "local", None, &store).unwrap();

        let spec = ModelSpec {
            name: "local".into(),
            source_url: "https://example.invalid/blob.gguf".into(),
            sha256_hex: HELLO_WORLD_SHA.into(),
            size_bytes: Some(11),
            license: None,
            source: None,
        };
        let got = fetch_model(&spec, &store, &dummy_broadcaster()).unwrap();
        assert_eq!(got, store.blob_path(HELLO_WORLD_SHA));
    }
}
