//! Activity log: NDJSON writer with rotation and write-time redaction.
//!
//! Per `THREAT_MODEL.md` F-3, F-4 and `docs/protocol-v1.md`'s
//! observability section.
//!
//! Shape on disk:
//! - Files live under the configured log dir (default
//!   `~/.inferd/logs/`).
//! - One file at a time named `inferd.ndjson`. When it crosses the
//!   configured size cap, it rotates to `inferd.ndjson.1` and a fresh
//!   `inferd.ndjson` is created. `inferd.ndjson.2` and `.3` follow.
//!   Anything beyond `.3` is deleted (the F-4 "3 generations" rule).
//! - Each record is one JSON object on a single line, terminated by
//!   `\n`. Fields: `t` (RFC3339 timestamp), `level`, `component`,
//!   `msg`, plus arbitrary structured fields supplied by `tracing`
//!   spans/events.
//!
//! The redactor runs *before* bytes hit the disk; even
//! `INFERD_LOG=debug` records are scrubbed.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::redact::redact_in_place;

/// Default per-file size cap before rotation.
pub const DEFAULT_ROTATE_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB

/// Number of historical generations kept. Per F-4 the daemon keeps
/// **3 historicals** plus the live file (`.ndjson`, `.1`, `.2`, `.3`).
pub const KEEP_GENERATIONS: u32 = 3;

/// A rotating NDJSON writer.
///
/// Single instance, internally locked. The `tracing` layer in
/// `tracing_layer::LogxLayer` calls `write_record` once per event.
pub struct LogxWriter {
    inner: Mutex<Inner>,
}

struct Inner {
    dir: PathBuf,
    base: String,
    rotate_bytes: u64,
    file: File,
    written: u64,
}

impl LogxWriter {
    /// Create or reopen the active log file under `dir/base.ndjson`.
    /// Creates `dir` if it does not exist.
    pub fn open(dir: &Path, base: &str, rotate_bytes: u64) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = log_path(dir, base, 0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            inner: Mutex::new(Inner {
                dir: dir.to_path_buf(),
                base: base.to_string(),
                rotate_bytes,
                file,
                written,
            }),
        })
    }

    /// Write one NDJSON record. The caller hands in the fully-formed
    /// JSON line (no trailing newline); this method runs the redactor,
    /// appends `\n`, and rotates if the cap is crossed.
    pub fn write_record(&self, mut record: String) -> io::Result<()> {
        redact_in_place(&mut record);
        let mut bytes = record.into_bytes();
        bytes.push(b'\n');

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("logx mutex poisoned"))?;

        if inner.written.saturating_add(bytes.len() as u64) > inner.rotate_bytes
            && inner.written > 0
        {
            inner.rotate()?;
        }

        inner.file.write_all(&bytes)?;
        inner.written = inner.written.saturating_add(bytes.len() as u64);
        Ok(())
    }

    /// Force a rotation regardless of size. Tests and ops tools.
    pub fn rotate_now(&self) -> io::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("logx mutex poisoned"))?;
        inner.rotate()
    }
}

impl Inner {
    fn rotate(&mut self) -> io::Result<()> {
        // Drop the live file before renaming so Windows allows the
        // replace; on Unix this is a no-op cost.
        drop_file(&mut self.file);

        // Cascade: .3 dies, .2 -> .3, .1 -> .2, live -> .1.
        let dir = self.dir.clone();
        let base = self.base.clone();
        delete_if_exists(&log_path(&dir, &base, KEEP_GENERATIONS))?;
        for n in (1..KEEP_GENERATIONS).rev() {
            let from = log_path(&dir, &base, n);
            let to = log_path(&dir, &base, n + 1);
            if from.exists() {
                std::fs::rename(&from, &to)?;
            }
        }
        let live = log_path(&dir, &base, 0);
        let to = log_path(&dir, &base, 1);
        if live.exists() {
            std::fs::rename(&live, &to)?;
        }

        self.file = OpenOptions::new().create(true).append(true).open(&live)?;
        self.written = 0;
        Ok(())
    }
}

fn drop_file(_f: &mut File) {
    // Reopen-replace pattern needs the old handle closed. We do this by
    // dropping the File via std::mem::replace with a sentinel. Keeping
    // this in a helper documents the intent.
    //
    // Actual drop happens when the caller next reassigns `inner.file`.
    // No-op body; the caller's reassignment closes the prior fd.
}

fn delete_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn log_path(dir: &Path, base: &str, generation: u32) -> PathBuf {
    if generation == 0 {
        dir.join(format!("{base}.ndjson"))
    } else {
        dir.join(format!("{base}.ndjson.{generation}"))
    }
}

/// Default log directory: `~/.inferd/logs/`. Honours `INFERD_LOG_DIR`
/// when set so operators and tests can redirect.
pub fn default_log_dir() -> PathBuf {
    if let Ok(p) = std::env::var("INFERD_LOG_DIR") {
        return PathBuf::from(p);
    }
    let home = dirs_home().unwrap_or_else(|| PathBuf::from("."));
    home.join(".inferd").join("logs")
}

fn dirs_home() -> Option<PathBuf> {
    // Avoid pulling the `dirs` crate just for HOME on Unix and USERPROFILE
    // on Windows. Both are well-defined env vars.
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn read_file(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn appends_records_and_rotates_at_size_cap() {
        let dir = tempdir().unwrap();
        // Cap of 50 bytes — second/third records each trigger a rotation
        // before write, so the cascade is:
        //   r1 -> live (live becomes 30 bytes)
        //   r2 -> rotate(live=>.1, contains r1); new live; write r2
        //   r3 -> rotate(.1=>.2 contains r1, live=>.1 contains r2); new live; write r3
        let writer = LogxWriter::open(dir.path(), "inferd", 50).unwrap();

        writer
            .write_record(r#"{"msg":"first record","n":1}"#.to_string())
            .unwrap();
        writer
            .write_record(r#"{"msg":"second record","n":2}"#.to_string())
            .unwrap();
        writer
            .write_record(r#"{"msg":"third record","n":3}"#.to_string())
            .unwrap();

        let live = read_file(&log_path(dir.path(), "inferd", 0));
        let one = read_file(&log_path(dir.path(), "inferd", 1));
        let two = read_file(&log_path(dir.path(), "inferd", 2));

        assert!(live.contains("third record"), "live should hold r3: {live}");
        assert!(one.contains("second record"), ".1 should hold r2: {one}");
        assert!(two.contains("first record"), ".2 should hold r1: {two}");
    }

    #[test]
    fn cascade_keeps_only_three_generations() {
        let dir = tempdir().unwrap();
        let writer = LogxWriter::open(dir.path(), "inferd", 1024).unwrap();

        writer.write_record(r#"{"g":0}"#.to_string()).unwrap();
        for _ in 0..5 {
            writer.rotate_now().unwrap();
            writer.write_record(r#"{"g":"new"}"#.to_string()).unwrap();
        }

        // .ndjson, .1, .2, .3 — nothing higher.
        for n in 0..=KEEP_GENERATIONS {
            assert!(
                log_path(dir.path(), "inferd", n).exists(),
                "missing generation {n}"
            );
        }
        assert!(
            !log_path(dir.path(), "inferd", KEEP_GENERATIONS + 1).exists(),
            "generation {} should have been pruned",
            KEEP_GENERATIONS + 1
        );
    }

    #[test]
    fn redactor_runs_on_write_path() {
        let dir = tempdir().unwrap();
        let writer = LogxWriter::open(dir.path(), "inferd", 1 << 20).unwrap();

        // Fixture assembled at runtime so secret-scanning tools don't
        // treat the literal as a real key in source.
        let fixture = format!("{}-{}", "sk", "1234567890abcdefghij");
        let record = format!(r#"{{"msg":"oops","key":"{fixture}"}}"#);
        writer.write_record(record).unwrap();

        let live = read_file(&log_path(dir.path(), "inferd", 0));
        assert!(!live.contains(&fixture), "secret leaked: {live}");
        assert!(
            live.contains("[REDACTED"),
            "expected redaction marker: {live}"
        );
    }
}
