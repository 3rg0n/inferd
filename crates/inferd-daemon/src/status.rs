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
    /// `inferdctl doctor`, IDE plugins) discover hardware-acceleration
    /// posture, multimodal support, and tools / thinking support
    /// without trial-and-error. Backwards-additive on the admin wire
    /// (older subscribers ignore unknown `status` values).
    Capabilities {
        /// Backend identifier (`"llamacpp"`, `"mock"`, …).
        backend: String,
        /// Wire-format version this daemon speaks (ADR 0021). Consumers
        /// compare against their own to fail loudly on a mismatch
        /// rather than mis-frame. Backwards-additive on the admin wire.
        wire_version: u32,
        /// `true` if the backend implements `generate_v2`.
        v2: bool,
        /// `true` if the backend can ingest image attachments.
        vision: bool,
        /// `true` if the backend can ingest audio attachments.
        audio: bool,
        /// Sample rate in Hz that the backend's audio encoder requires
        /// for `Attachment::Audio` payloads. Omitted when the backend
        /// ingests no audio or reports no rate.
        ///
        /// Consumers must send PCM at exactly this rate: the daemon does
        /// not resample (ADR 0016 — the consumer decodes, so it owns rate
        /// conversion), and a mismatched rate is rejected rather than
        /// mis-fed to the encoder. This field is what makes the required
        /// rate discoverable instead of guesswork. Backwards-additive:
        /// older subscribers ignore it.
        #[serde(skip_serializing_if = "Option::is_none")]
        audio_sample_rate: Option<u32>,
        /// `true` if the backend natively supports tool-use.
        tools: bool,
        /// `true` if the backend separates `<|think|>` reasoning
        /// trace from user-visible output.
        thinking: bool,
        /// `true` if the backend implements `embed` (per ADR 0017).
        /// Subscribers use this to decide whether to expose embedding
        /// surfaces in their UIs.
        embed: bool,
        /// Active GGML accelerator: `"cpu"` / `"cuda"` / `"metal"` /
        /// `"vulkan"` / `"rocm"`. In `dl-backends` builds (v0.3+),
        /// runtime-probed; in v0.2.x static builds, compile-time.
        accelerator: String,
        /// Layers offloaded to the accelerator at runtime. 0 means
        /// CPU-only regardless of `accelerator`.
        gpu_layers: u32,
        /// Human-readable device name reported by ggml (e.g.
        /// `"NVIDIA GeForce RTX 4090"`, `"Apple M2 Pro"`). `None` on
        /// the CPU path or when the active backend exposes no device
        /// (e.g. cloud adapters). Backwards-additive: older
        /// subscribers ignore unknown fields.
        #[serde(skip_serializing_if = "Option::is_none")]
        device_name: Option<String>,
        /// Total VRAM (or unified memory budget on Apple Silicon) in
        /// bytes. `None` when the backend doesn't report a total.
        /// Free VRAM is intentionally omitted — it changes
        /// second-to-second and would force a re-probe on every emit.
        #[serde(skip_serializing_if = "Option::is_none")]
        vram_total_bytes: Option<u64>,
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

    /// Build a `Capabilities` event, varying only the audio fields.
    fn caps(audio: bool, audio_sample_rate: Option<u32>) -> StatusEvent {
        StatusEvent::Capabilities {
            backend: "llamacpp".into(),
            wire_version: inferd_proto::v2::WIRE_VERSION,
            v2: true,
            vision: true,
            audio,
            audio_sample_rate,
            tools: true,
            thinking: true,
            embed: false,
            accelerator: "cpu".into(),
            gpu_layers: 0,
            device_name: None,
            vram_total_bytes: None,
        }
    }

    #[test]
    fn capabilities_publishes_required_audio_sample_rate() {
        // The rate has to reach the admin wire, not just live in
        // BackendCapabilities: it is the only way a consumer can learn
        // what rate to resample to before sending PCM, since the daemon
        // rejects a mismatch rather than resampling (ADR 0016).
        let s = serde_json::to_value(caps(true, Some(16_000))).unwrap();
        assert_eq!(s["status"], "capabilities");
        assert_eq!(s["audio"], true);
        assert_eq!(s["audio_sample_rate"], 16_000);
    }

    #[test]
    fn capabilities_omits_audio_sample_rate_when_unknown() {
        // A non-audio backend must not emit a null rate — subscribers
        // that predate the field see the frame unchanged.
        let s = serde_json::to_value(caps(false, None)).unwrap();
        assert_eq!(s["audio"], false);
        assert!(
            s.get("audio_sample_rate").is_none(),
            "audio_sample_rate must be absent, not null: {s}"
        );
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
