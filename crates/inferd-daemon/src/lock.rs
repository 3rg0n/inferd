//! Single-instance lock for the daemon.
//!
//! See `THREAT_MODEL.md` F-2: pre-existing symlinks at the lock path are
//! rejected before any open is attempted. The lock itself is acquired via
//! `std::fs::File::try_lock` (stable since Rust 1.89), which uses
//! `flock(LOCK_EX|LOCK_NB)` on Unix and `LockFileEx` with
//! `LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY` on Windows.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Errors produced when acquiring the single-instance lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// The lock path pre-exists as a symlink. Refused per F-2.
    #[error("lock path is a symlink (refused): {0}")]
    Symlink(PathBuf),
    /// The lock path exists and is not a regular file (e.g. a directory).
    #[error("lock path is not a regular file: {0}")]
    NotARegularFile(PathBuf),
    /// Another daemon already holds the lock.
    #[error("another inferd-daemon already holds the lock at {0}")]
    AlreadyHeld(PathBuf),
    /// Underlying I/O error.
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

/// An acquired single-instance lock.
///
/// The lock is released when this value is dropped (closing the file
/// descriptor releases the OS-level lock). The lock file itself is **not**
/// deleted on drop — leaving stale lock files on disk after a crash is
/// normal and harmless because the lock is held by the open fd, not by the
/// file's mere existence.
#[derive(Debug)]
pub struct Lock {
    _file: File,
    path: PathBuf,
}

impl Lock {
    /// Acquire the single-instance lock at `path`.
    ///
    /// The function:
    /// 1. Calls `symlink_metadata` and refuses if the path is a symlink (F-2).
    /// 2. Opens the file with `O_NOFOLLOW` semantics (via fresh metadata
    ///    check then create-or-open).
    /// 3. Calls `try_lock_exclusive`. If another process holds the lock,
    ///    returns `LockError::AlreadyHeld`.
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = path.as_ref();

        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                return Err(LockError::Symlink(path.to_path_buf()));
            }
            if !meta.file_type().is_file() {
                return Err(LockError::NotARegularFile(path.to_path_buf()));
            }
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        match file.try_lock() {
            Ok(()) => Ok(Lock {
                _file: file,
                path: path.to_path_buf(),
            }),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err(LockError::AlreadyHeld(path.to_path_buf()))
            }
            Err(std::fs::TryLockError::Error(e)) => Err(LockError::Io(e)),
        }
    }

    /// Path of the lock file (for diagnostics and tests).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn acquire_then_release_succeeds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("inferd.lock");
        let lock = Lock::acquire(&path).unwrap();
        assert_eq!(lock.path(), &path);
        drop(lock);
        // Re-acquire after drop must succeed.
        let _again = Lock::acquire(&path).unwrap();
    }

    #[test]
    fn second_acquire_while_held_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("inferd.lock");
        let _first = Lock::acquire(&path).unwrap();
        let err = Lock::acquire(&path).unwrap_err();
        assert!(matches!(err, LockError::AlreadyHeld(_)));
    }

    // F-2: pre-existing symlinks are refused.
    //
    // Skipped on Windows because creating a symlink requires either developer
    // mode or elevated privileges; the underlying invariant is identical
    // (the same `is_symlink()` check runs on every platform).
    #[cfg(unix)]
    #[test]
    fn pre_existing_symlink_is_refused() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.bin");
        std::fs::write(&target, b"x").unwrap();
        let symlink = dir.path().join("inferd.lock");
        std::os::unix::fs::symlink(&target, &symlink).unwrap();

        let err = Lock::acquire(&symlink).unwrap_err();
        assert!(matches!(err, LockError::Symlink(_)));
    }

    #[test]
    fn directory_at_lock_path_refused() {
        let dir = tempdir().unwrap();
        let bad = dir.path().join("inferd.lock");
        std::fs::create_dir(&bad).unwrap();
        let err = Lock::acquire(&bad).unwrap_err();
        assert!(matches!(err, LockError::NotARegularFile(_)));
    }
}
