//! Admin-socket subscriber. Read-only stream of lifecycle events
//! from the daemon. Per `docs/protocol-v1.md` §"Admin endpoint".

use serde::Deserialize;
use std::io;
#[cfg(unix)]
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// One frame off the admin socket. Fields not relevant to the
/// current `status`/`phase` are absent (or default) per the spec's
/// flattened wire shape.
///
/// Forward compatibility: clients **must ignore** unknown `status`,
/// `phase`, and detail keys per `docs/protocol-v1.md`. Unknown
/// values land in the typed fields verbatim — branch on values you
/// recognise; default to logging-and-ignoring otherwise.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AdminEvent {
    /// Always `"admin"` in v1.
    #[serde(default)]
    pub id: String,
    /// Always `"status"` in v1.
    #[serde(default)]
    #[serde(rename = "type")]
    pub kind: String,
    /// One of `starting`, `loading_model`, `ready`, `restarting`,
    /// `draining`. Unknown values surface verbatim — ignore them.
    pub status: String,
    /// Set on `loading_model` and `restarting`. One of
    /// `checking_local`, `download`, `verify`, `quarantine`,
    /// `mmap`, `kv_cache`. Unknown values surface verbatim.
    #[serde(default)]
    pub phase: String,

    // --- Phase-specific detail fields (flattened on the wire) ---
    /// Path being checked / verified / mmapped / quarantined.
    #[serde(default)]
    pub path: Option<String>,
    /// Bytes downloaded so far (download phase).
    #[serde(default)]
    pub downloaded_bytes: Option<u64>,
    /// Total bytes if known (download phase). `None` when the
    /// server didn't supply Content-Length.
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// Source URL (download phase). Diagnostic only.
    #[serde(default)]
    pub source_url: Option<String>,
    /// Expected SHA-256 (quarantine phase).
    #[serde(default)]
    pub expected_sha256: Option<String>,
    /// Computed SHA-256 (quarantine phase).
    #[serde(default)]
    pub actual_sha256: Option<String>,
    /// Where the bad bytes were moved (quarantine phase).
    #[serde(default)]
    pub quarantine_path: Option<String>,
    /// Configured context window in tokens (kv_cache phase).
    #[serde(default)]
    pub n_ctx: Option<u32>,

    // --- capabilities-specific fields (status="capabilities", #77) ---
    /// Backend identifier (capabilities phase).
    #[serde(default)]
    pub backend: Option<String>,
    /// Wire-format version the daemon speaks (capabilities phase,
    /// ADR 0021). Compare against the client's own to fail loudly on a
    /// mismatch.
    #[serde(default)]
    pub wire_version: Option<u32>,
    /// `true` if the backend implements v2 (capabilities phase).
    #[serde(default)]
    pub v2: Option<bool>,
    /// `true` if the backend can ingest images (capabilities phase).
    #[serde(default)]
    pub vision: Option<bool>,
    /// `true` if the backend can ingest audio (capabilities phase).
    #[serde(default)]
    pub audio: Option<bool>,
    /// `true` if the backend natively supports tool-use
    /// (capabilities phase).
    #[serde(default)]
    pub tools: Option<bool>,
    /// `true` if the backend separates `<|think|>` reasoning from
    /// user-visible output (capabilities phase).
    #[serde(default)]
    pub thinking: Option<bool>,
    /// `true` if the backend implements `embed` per ADR 0017
    /// (capabilities phase).
    #[serde(default)]
    pub embed: Option<bool>,
    /// Active GGML accelerator: `"cpu"` / `"cuda"` / `"metal"` /
    /// `"vulkan"` / `"rocm"` (capabilities phase). Runtime-probed in
    /// `dl-backends` builds (v0.3+); compile-time in v0.2.x.
    #[serde(default)]
    pub accelerator: Option<String>,
    /// Layers offloaded to the accelerator at runtime (capabilities
    /// phase). 0 means CPU-only at runtime regardless of `accelerator`.
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    /// Human-readable device name reported by ggml (capabilities
    /// phase). `None` on the CPU path or when the active backend
    /// exposes no device.
    #[serde(default)]
    pub device_name: Option<String>,
    /// Total VRAM (or unified memory budget) in bytes (capabilities
    /// phase). `None` when the backend doesn't report a total.
    #[serde(default)]
    pub vram_total_bytes: Option<u64>,
}

/// Errors produced by the admin client.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// JSON decode of an admin frame failed.
    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),
    /// Daemon closed the admin socket. Reconnect to resume.
    #[error("admin socket closed")]
    Closed,
}

/// Subscriber for the inferd admin socket.
///
/// Construct via `dial_admin_uds` (Unix) / `dial_admin_pipe`
/// (Windows). Read events with `recv()` or wait for a specific
/// state with `wait_ready()`.
pub struct AdminClient {
    reader: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

impl AdminClient {
    /// Open a Unix domain socket connection to the admin socket
    /// (Unix only).
    #[cfg(unix)]
    pub async fn dial_admin_uds(path: &Path) -> Result<Self, AdminError> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(Self {
            reader: BufReader::with_capacity(8192, Box::new(stream)),
        })
    }

    /// Open a Windows named pipe connection to the admin socket
    /// (Windows only).
    #[cfg(windows)]
    pub async fn dial_admin_pipe(path: &str) -> Result<Self, AdminError> {
        use tokio::net::windows::named_pipe::ClientOptions;
        let pipe = ClientOptions::new().open(path)?;
        Ok(Self {
            reader: BufReader::with_capacity(8192, Box::new(pipe)),
        })
    }

    /// Read the next admin event. Blocks until a frame arrives or
    /// the daemon closes the connection.
    pub async fn recv(&mut self) -> Result<AdminEvent, AdminError> {
        let mut line = Vec::with_capacity(512);
        let n = self.reader.read_until(b'\n', &mut line).await?;
        if n == 0 {
            return Err(AdminError::Closed);
        }
        let event: AdminEvent = serde_json::from_slice(&line)?;
        Ok(event)
    }

    /// Loop `recv()` until a `ready` event arrives. Returns the
    /// event that flipped state. Returns `Err(AdminError::Closed)`
    /// if the daemon closes before reaching `ready` (a daemon that
    /// crashes mid-load looks like this).
    pub async fn wait_ready(&mut self) -> Result<AdminEvent, AdminError> {
        loop {
            let ev = self.recv().await?;
            if ev.status == "ready" {
                return Ok(ev);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_download_frame() {
        let raw = br#"{
            "id":"admin","type":"status","status":"loading_model","phase":"download",
            "downloaded_bytes":33554432,"total_bytes":5126304928,
            "source_url":"https://huggingface.co/example.gguf"
        }"#;
        let ev: AdminEvent = serde_json::from_slice(raw).unwrap();
        assert_eq!(ev.id, "admin");
        assert_eq!(ev.kind, "status");
        assert_eq!(ev.status, "loading_model");
        assert_eq!(ev.phase, "download");
        assert_eq!(ev.downloaded_bytes, Some(33_554_432));
        assert_eq!(ev.total_bytes, Some(5_126_304_928));
        assert_eq!(
            ev.source_url.as_deref(),
            Some("https://huggingface.co/example.gguf")
        );
    }

    #[test]
    fn decodes_ready_frame() {
        let raw = br#"{"id":"admin","type":"status","status":"ready"}"#;
        let ev: AdminEvent = serde_json::from_slice(raw).unwrap();
        assert_eq!(ev.status, "ready");
        assert_eq!(ev.phase, "");
        assert!(ev.downloaded_bytes.is_none());
    }

    #[test]
    fn total_bytes_may_be_null() {
        let raw = br#"{"id":"admin","type":"status","status":"loading_model","phase":"download","downloaded_bytes":1024,"total_bytes":null,"source_url":"https://x"}"#;
        let ev: AdminEvent = serde_json::from_slice(raw).unwrap();
        assert_eq!(ev.downloaded_bytes, Some(1024));
        assert_eq!(ev.total_bytes, None);
    }

    #[test]
    fn unknown_status_round_trips_verbatim() {
        let raw = br#"{"id":"admin","type":"status","status":"future_state_we_dont_know","extra_key":42}"#;
        let ev: AdminEvent = serde_json::from_slice(raw).unwrap();
        assert_eq!(ev.status, "future_state_we_dont_know");
    }

    #[tokio::test]
    async fn recv_decodes_from_a_duplex_pipe() {
        let (mut server_side, client_side) = tokio::io::duplex(4096);
        use tokio::io::AsyncWriteExt;
        server_side
            .write_all(b"{\"id\":\"admin\",\"type\":\"status\",\"status\":\"starting\"}\n")
            .await
            .unwrap();
        server_side.flush().await.unwrap();

        let mut client = AdminClient {
            reader: BufReader::with_capacity(4096, Box::new(client_side)),
        };
        let ev = client.recv().await.unwrap();
        assert_eq!(ev.status, "starting");
    }

    #[tokio::test]
    async fn wait_ready_skips_loading_frames() {
        let (mut server_side, client_side) = tokio::io::duplex(4096);
        use tokio::io::AsyncWriteExt;
        let writes = b"\
{\"id\":\"admin\",\"type\":\"status\",\"status\":\"starting\"}\n\
{\"id\":\"admin\",\"type\":\"status\",\"status\":\"loading_model\",\"phase\":\"checking_local\",\"path\":\"/x.gguf\"}\n\
{\"id\":\"admin\",\"type\":\"status\",\"status\":\"loading_model\",\"phase\":\"mmap\",\"path\":\"/x.gguf\"}\n\
{\"id\":\"admin\",\"type\":\"status\",\"status\":\"ready\"}\n\
";
        server_side.write_all(writes).await.unwrap();
        server_side.flush().await.unwrap();

        let mut client = AdminClient {
            reader: BufReader::with_capacity(4096, Box::new(client_side)),
        };
        let ev = client.wait_ready().await.unwrap();
        assert_eq!(ev.status, "ready");
    }

    #[tokio::test]
    async fn recv_reports_closed_on_eof() {
        let (server_side, client_side) = tokio::io::duplex(4096);
        drop(server_side);

        let mut client = AdminClient {
            reader: BufReader::with_capacity(4096, Box::new(client_side)),
        };
        match client.recv().await {
            Err(AdminError::Closed) => {}
            other => panic!("expected Closed, got {other:?}"),
        }
    }
}
