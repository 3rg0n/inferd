//! Daemon-wide status events.
//!
//! Published by bring-up code (fetch, lifecycle, etc.) through a
//! `tokio::sync::broadcast` channel. The admin socket subscribes
//! and serialises each event into the `Status` frame shape
//! documented in `docs/protocol-v1.md` §"Admin socket".
//!
//! Frame shape on the wire (NDJSON):
//!
//! ```json
//! {"id":"admin","type":"status","status":"loading_model",
//!  "phase":"download",
//!  "detail":{"downloaded_bytes":33554432,"total_bytes":5126304928,
//!            "source_url":"https://huggingface.co/..."}}
//! ```
//!
//! These types are the in-process representation; serialisation is
//! done by `admin.rs` via `serde_json` into the wire shape above.

use serde::Serialize;
use std::path::PathBuf;

/// One daemon-wide status event. Published through a broadcast channel
/// to admin-socket subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StatusEvent {
    /// Daemon process is up; admin socket is bound. No backend work yet.
    Starting,
    /// Model is being prepared. Carries a `phase` describing the
    /// sub-stage and (for download) progress numbers.
    LoadingModel {
        /// Sub-stage of model bring-up. Flattened on the wire so the
        /// frame reads
        /// `{"status":"loading_model","phase":"download","downloaded_bytes":...}`
        /// rather than nesting the phase under another object.
        #[serde(flatten)]
        phase: LoadPhase,
    },
    /// Backend capability snapshot — emitted once after the backend is
    /// constructed and before `Ready`. Lets admin subscribers (e.g.
    /// `inferd doctor`, IDE plugins) discover hardware-acceleration
    /// posture, multimodal support, and tools / thinking support
    /// without trial-and-error. Backwards-additive on the admin wire
    /// (older subscribers ignore unknown `status` values).
    Capabilities {
        /// Backend identifier (`"llamacpp"`, `"mock"`, …).
        backend: String,
        /// `true` if the backend implements `generate_v2`.
        v2: bool,
        /// `true` if the backend can ingest image attachments.
        vision: bool,
        /// `true` if the backend can ingest audio attachments.
        audio: bool,
        /// `true` if the backend natively supports tool-use.
        tools: bool,
        /// `true` if the backend separates `<|think|>` reasoning
        /// trace from user-visible output.
        thinking: bool,
        /// Compile-time GGML accelerator: `"cpu"` / `"cuda"` / `"metal"`
        /// / `"vulkan"` / `"rocm"`.
        accelerator: String,
        /// Layers offloaded to the accelerator at runtime. 0 means
        /// CPU-only regardless of `accelerator`.
        gpu_layers: u32,
    },
    /// Inference socket is bound and accepting connections.
    Ready,
    /// Previously-`ready` daemon is reloading. Inference socket has
    /// closed; new connections refused. Carries the same `phase` enum
    /// as `LoadingModel` so subscribers can show progress for the
    /// reload.
    Restarting {
        /// Sub-stage of the restart.
        #[serde(flatten)]
        phase: LoadPhase,
    },
    /// Daemon received a shutdown signal. Existing requests finish;
    /// new requests rejected. Daemon will exit shortly after this
    /// frame.
    Draining,
}

/// Sub-stage of a model load (or reload).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum LoadPhase {
    /// Resolving model path on disk and checking SHA-256.
    CheckingLocal {
        /// Absolute path being checked.
        path: PathBuf,
    },
    /// Downloading the GGUF. Progress events emit periodically.
    Download {
        /// Bytes received so far.
        downloaded_bytes: u64,
        /// Total bytes if known (Content-Length or config), else `None`.
        total_bytes: Option<u64>,
        /// URL being fetched (for diagnostics only; never echoed to
        /// inference clients — admin socket only).
        source_url: String,
    },
    /// Streaming SHA-256 over downloaded bytes for final verification.
    Verify {
        /// Path being verified.
        path: PathBuf,
    },
    /// Downloaded bytes failed SHA verification; file moved aside.
    /// Daemon will retry or exit per `auto_pull` config.
    Quarantine {
        /// Original path of the bad file.
        path: PathBuf,
        /// What the config said the SHA should be.
        expected_sha256: String,
        /// What we actually computed.
        actual_sha256: String,
        /// Where the bad bytes were moved.
        quarantine_path: PathBuf,
    },
    /// Loading the file into the engine via FFI.
    Mmap {
        /// Path being mmapped.
        path: PathBuf,
    },
    /// Allocating the KV cache.
    KvCache {
        /// Configured context window in tokens.
        n_ctx: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_serialises_as_simple_status() {
        let s = serde_json::to_value(StatusEvent::Starting).unwrap();
        assert_eq!(s["status"], "starting");
    }

    #[test]
    fn ready_serialises_as_simple_status() {
        let s = serde_json::to_value(StatusEvent::Ready).unwrap();
        assert_eq!(s["status"], "ready");
    }

    #[test]
    fn loading_model_download_carries_progress() {
        let ev = StatusEvent::LoadingModel {
            phase: LoadPhase::Download {
                downloaded_bytes: 33_554_432,
                total_bytes: Some(5_126_304_928),
                source_url: "https://example.com/x.gguf".into(),
            },
        };
        let s = serde_json::to_value(ev).unwrap();
        assert_eq!(s["status"], "loading_model");
        assert_eq!(s["phase"], "download");
        assert_eq!(s["downloaded_bytes"], 33_554_432);
        assert_eq!(s["total_bytes"], 5_126_304_928u64);
        assert_eq!(s["source_url"], "https://example.com/x.gguf");
    }

    #[test]
    fn loading_model_checking_local_carries_path() {
        let ev = StatusEvent::LoadingModel {
            phase: LoadPhase::CheckingLocal {
                path: PathBuf::from("/tmp/x.gguf"),
            },
        };
        let s = serde_json::to_value(ev).unwrap();
        assert_eq!(s["status"], "loading_model");
        assert_eq!(s["phase"], "checking_local");
        // Path serialises platform-natively; on Unix this is "/tmp/x.gguf".
        assert!(s["path"].is_string());
    }

    #[test]
    fn quarantine_carries_both_hashes() {
        let ev = StatusEvent::LoadingModel {
            phase: LoadPhase::Quarantine {
                path: PathBuf::from("/tmp/x.gguf"),
                expected_sha256: "aaaa".into(),
                actual_sha256: "bbbb".into(),
                quarantine_path: PathBuf::from("/tmp/x.gguf.quarantine.20260518T000000Z"),
            },
        };
        let s = serde_json::to_value(ev).unwrap();
        assert_eq!(s["phase"], "quarantine");
        assert_eq!(s["expected_sha256"], "aaaa");
        assert_eq!(s["actual_sha256"], "bbbb");
    }
}
