//! Admin socket — push-style daemon-lifecycle event broadcast.
//!
//! Specified in `docs/protocol-v1.md` §"Admin endpoint". On connect,
//! the daemon writes a snapshot frame (current state). Every state
//! transition pushes another frame. Read-only stream from daemon →
//! client; client writes are ignored.
//!
//! Wire shape:
//!
//! ```json
//! {"id":"admin","type":"status","status":"loading_model","phase":"download",
//!  "downloaded_bytes":33554432,"total_bytes":5126304928,
//!  "source_url":"https://huggingface.co/..."}
//! ```
//!
//! Architecture:
//!
//! - `StatusBroadcaster` wraps a `tokio::sync::watch` (for the
//!   "current state" snapshot newcomers see) and a
//!   `tokio::sync::broadcast` (for the live event stream).
//! - `serve_admin_*` accept loops per platform; each accepted
//!   connection spawns a writer task that (1) writes the watch's
//!   current value as the snapshot, then (2) forwards every
//!   broadcast event after that. Slow clients overflow the
//!   broadcast buffer and get disconnected with EOF — they
//!   reconnect to resume.

use crate::status::StatusEvent;
use serde_json::json;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

/// Capacity of the broadcast channel that fans events out to admin
/// clients. Past this, slow clients lag behind and the daemon drops
/// them — they reconnect to resume from the current snapshot.
pub const ADMIN_BROADCAST_CAPACITY: usize = 256;

/// Daemon-wide status broadcaster.
///
/// Producers (fetch, lifecycle, etc.) call `publish` to update the
/// current state. Subscribers (admin clients) connect to the admin
/// socket; each gets a snapshot of the current state on connect, then
/// the live event stream.
#[derive(Clone)]
pub struct StatusBroadcaster {
    /// `watch` carries the *most recent* event so newcomers don't
    /// need to wait for the next state change.
    snapshot: watch::Sender<StatusEvent>,
    /// `watch` carries the most recent `Capabilities` event so
    /// late-connecting one-shot subscribers (e.g. `inferdctl doctor`)
    /// see capability info even though it was published once at boot.
    /// `None` until the backend has been constructed.
    capabilities: watch::Sender<Option<StatusEvent>>,
    /// `broadcast` carries the live event stream. Subscribers receive
    /// every event from the moment they subscribe.
    events: broadcast::Sender<StatusEvent>,
}

impl StatusBroadcaster {
    /// Build a fresh broadcaster seeded with `initial`. Typical
    /// initial value is `StatusEvent::Starting`.
    pub fn new(initial: StatusEvent) -> Self {
        let (snapshot, _rx) = watch::channel(initial);
        let (capabilities, _rx) = watch::channel(None);
        let (events, _rx) = broadcast::channel(ADMIN_BROADCAST_CAPACITY);
        Self {
            snapshot,
            capabilities,
            events,
        }
    }

    /// Publish a new state. Updates the snapshot (so subsequent
    /// connects see this) AND fans out to all current subscribers.
    pub fn publish(&self, event: StatusEvent) {
        // `send_replace` updates the watch's stored value regardless
        // of how many receivers are alive — `send` would silently
        // drop the value if no one is currently subscribed, which
        // is wrong for snapshot semantics.
        if matches!(event, StatusEvent::Capabilities { .. }) {
            let _ = self.capabilities.send_replace(Some(event.clone()));
        } else {
            let _ = self.snapshot.send_replace(event.clone());
        }
        let _ = self.events.send(event);
    }

    /// Snapshot of the current state. Used by the admin accept loop
    /// to write the first frame on connect.
    pub fn current(&self) -> StatusEvent {
        self.snapshot.borrow().clone()
    }

    /// Most recent capability advertisement, if any. The admin accept
    /// loop writes this *before* the snapshot frame so one-shot
    /// readers see capabilities even when they connect after Ready.
    pub fn latest_capabilities(&self) -> Option<StatusEvent> {
        self.capabilities.borrow().clone()
    }

    /// Subscribe to the live event stream. The receiver yields every
    /// event published *after* this call.
    pub fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.events.subscribe()
    }
}

/// Serialise a single `StatusEvent` into the admin wire shape:
/// `{"id":"admin","type":"status",...}` plus a trailing newline.
fn render_frame(event: &StatusEvent) -> Vec<u8> {
    // `serde(tag = "status", flatten phase)` already produces the
    // status + phase + detail structure; we wrap it with the admin
    // envelope (`id`, `type`).
    let body = serde_json::to_value(event).unwrap_or_else(|_| json!({"status": "error"}));
    let mut envelope = serde_json::Map::new();
    envelope.insert("id".into(), json!("admin"));
    envelope.insert("type".into(), json!("status"));
    if let Some(obj) = body.as_object() {
        for (k, v) in obj {
            envelope.insert(k.clone(), v.clone());
        }
    }
    let mut bytes = serde_json::to_vec(&serde_json::Value::Object(envelope)).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// Drive one accepted admin connection: write the snapshot frame,
/// then forward every subsequent broadcast event until the client
/// disconnects or the broadcast lags out (slow consumer).
///
/// The connection is read-only from the client's perspective; this
/// function never reads from the stream.
async fn handle_admin_connection<W: AsyncWrite + Unpin>(
    mut writer: W,
    snapshot: StatusEvent,
    capabilities: Option<StatusEvent>,
    mut rx: broadcast::Receiver<StatusEvent>,
) -> io::Result<()> {
    // 1a. Capability frame (if known) — written before the snapshot
    // so one-shot subscribers see it.
    if let Some(caps) = capabilities {
        writer.write_all(&render_frame(&caps)).await?;
    }
    // 1b. Snapshot frame.
    writer.write_all(&render_frame(&snapshot)).await?;
    writer.flush().await?;

    // 2. Live event stream.
    loop {
        match rx.recv().await {
            Ok(event) => {
                if writer.write_all(&render_frame(&event)).await.is_err() {
                    return Ok(()); // peer closed
                }
                if writer.flush().await.is_err() {
                    return Ok(());
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "admin client lagged broadcast; closing");
                return Ok(());
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

/// Serve an admin Unix domain socket (Unix only). Loops until
/// `shutdown` resolves.
#[cfg(unix)]
pub async fn serve_admin_uds(
    listener: tokio::net::UnixListener,
    broadcaster: Arc<StatusBroadcaster>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    info!("admin uds listener accepting");
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("admin shutdown signalled");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let snapshot = broadcaster.current();
                let capabilities = broadcaster.latest_capabilities();
                let rx = broadcaster.subscribe();
                debug!("admin uds accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_admin_connection(stream, snapshot, capabilities, rx).await {
                        debug!(error = ?e, "admin connection ended with error");
                    }
                });
            }
        }
    }
}

/// Serve an admin Windows named pipe (Windows only). Mirrors the
/// inference-pipe multi-instance accept pattern: caller pre-binds
/// the first instance, the loop binds the next one before spawning
/// the handler.
#[cfg(windows)]
pub async fn serve_admin_pipe(
    path: &str,
    first_instance: tokio::net::windows::named_pipe::NamedPipeServer,
    broadcaster: Arc<StatusBroadcaster>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> io::Result<()> {
    use crate::endpoint::bind_admin_pipe;

    info!(path = %path, "admin pipe listener accepting");
    let mut server = first_instance;
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                info!("admin shutdown signalled");
                return Ok(());
            }
            connect_result = server.connect() => {
                connect_result?;
                let connected = server;
                server = bind_admin_pipe(path, false)?;

                let snapshot = broadcaster.current();
                let capabilities = broadcaster.latest_capabilities();
                let rx = broadcaster.subscribe();
                debug!("admin pipe accept");
                tokio::spawn(async move {
                    if let Err(e) = handle_admin_connection(connected, snapshot, capabilities, rx).await {
                        debug!(error = ?e, "admin connection ended with error");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::LoadPhase;
    use std::path::PathBuf;
    use std::time::Duration;

    fn parse_admin_frame(line: &[u8]) -> serde_json::Value {
        let trimmed = std::str::from_utf8(line).unwrap().trim_end_matches('\n');
        serde_json::from_str(trimmed).unwrap()
    }

    #[test]
    fn render_frame_wraps_with_admin_envelope() {
        let bytes = render_frame(&StatusEvent::Ready);
        let v = parse_admin_frame(&bytes);
        assert_eq!(v["id"], "admin");
        assert_eq!(v["type"], "status");
        assert_eq!(v["status"], "ready");
    }

    #[test]
    fn render_frame_flattens_loading_model_phase() {
        let bytes = render_frame(&StatusEvent::LoadingModel {
            phase: LoadPhase::Download {
                downloaded_bytes: 33_554_432,
                total_bytes: Some(5_126_304_928),
                source_url: "https://example.com/x.gguf".into(),
            },
        });
        let v = parse_admin_frame(&bytes);
        assert_eq!(v["id"], "admin");
        assert_eq!(v["type"], "status");
        assert_eq!(v["status"], "loading_model");
        assert_eq!(v["phase"], "download");
        assert_eq!(v["downloaded_bytes"], 33_554_432);
        assert_eq!(v["total_bytes"], 5_126_304_928u64);
        assert_eq!(v["source_url"], "https://example.com/x.gguf");
    }

    #[tokio::test]
    async fn broadcaster_snapshot_returns_initial_state() {
        let b = StatusBroadcaster::new(StatusEvent::Starting);
        match b.current() {
            StatusEvent::Starting => {}
            other => panic!("expected Starting, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn broadcaster_publish_updates_snapshot_and_fans_out() {
        let b = StatusBroadcaster::new(StatusEvent::Starting);
        let mut rx1 = b.subscribe();
        let mut rx2 = b.subscribe();

        b.publish(StatusEvent::Ready);

        // Both subscribers see the event.
        match rx1.recv().await {
            Ok(StatusEvent::Ready) => {}
            other => panic!("rx1: expected Ready, got {other:?}"),
        }
        match rx2.recv().await {
            Ok(StatusEvent::Ready) => {}
            other => panic!("rx2: expected Ready, got {other:?}"),
        }

        // Snapshot reflects the latest publish.
        match b.current() {
            StatusEvent::Ready => {}
            other => panic!("expected snapshot Ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn capabilities_publish_does_not_overwrite_lifecycle_snapshot() {
        // The Capabilities event is stored in its own slot so that
        // late-connecting subscribers still see the latest lifecycle
        // state (Ready / Restarting / Draining) as their snapshot.
        let b = StatusBroadcaster::new(StatusEvent::Starting);
        b.publish(StatusEvent::Capabilities {
            backend: "llamacpp".into(),
            v2: true,
            vision: true,
            audio: false,
            tools: true,
            thinking: true,
            embed: false,
            accelerator: "cuda".into(),
            gpu_layers: 99,
            device_name: Some("NVIDIA GeForce RTX 4090".into()),
            vram_total_bytes: Some(24 * 1024 * 1024 * 1024),
        });
        // Snapshot is still Starting — Capabilities lives outside it.
        match b.current() {
            StatusEvent::Starting => {}
            other => panic!("expected Starting in snapshot, got {other:?}"),
        }
        // Capabilities is recorded for the connect prefix.
        match b.latest_capabilities() {
            Some(StatusEvent::Capabilities {
                backend,
                accelerator,
                gpu_layers,
                ..
            }) => {
                assert_eq!(backend, "llamacpp");
                assert_eq!(accelerator, "cuda");
                assert_eq!(gpu_layers, 99);
            }
            other => panic!("expected Capabilities, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_admin_connection_writes_capabilities_then_snapshot() {
        let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);
        let b = StatusBroadcaster::new(StatusEvent::Starting);
        b.publish(StatusEvent::Capabilities {
            backend: "llamacpp".into(),
            v2: true,
            vision: false,
            audio: false,
            tools: true,
            thinking: true,
            embed: false,
            accelerator: "cpu".into(),
            gpu_layers: 0,
            device_name: None,
            vram_total_bytes: None,
        });
        b.publish(StatusEvent::Ready);

        let snapshot = b.current();
        let capabilities = b.latest_capabilities();
        let rx = b.subscribe();
        let handle = tokio::spawn(async move {
            let _ = handle_admin_connection(server_side, snapshot, capabilities, rx).await;
        });

        use tokio::io::AsyncBufReadExt;
        let mut reader = tokio::io::BufReader::new(&mut client_side);

        // First frame: capabilities.
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).await.unwrap();
        assert!(n > 0);
        let v = parse_admin_frame(&line);
        assert_eq!(v["status"], "capabilities");
        assert_eq!(v["backend"], "llamacpp");
        assert_eq!(v["accelerator"], "cpu");
        assert_eq!(v["gpu_layers"], 0);

        // Second frame: snapshot (Ready).
        let mut line2 = Vec::new();
        let n2 = reader.read_until(b'\n', &mut line2).await.unwrap();
        assert!(n2 > 0);
        let v2 = parse_admin_frame(&line2);
        assert_eq!(v2["status"], "ready");

        drop(client_side);
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn handle_admin_connection_writes_snapshot_first() {
        // Use a duplex pipe so we can read what the handler wrote
        // without binding a real socket.
        let (server_side, mut client_side) = tokio::io::duplex(64 * 1024);
        let b = StatusBroadcaster::new(StatusEvent::Starting);
        b.publish(StatusEvent::LoadingModel {
            phase: LoadPhase::CheckingLocal {
                path: PathBuf::from("/tmp/x.gguf"),
            },
        });

        let snapshot = b.current();
        let capabilities = b.latest_capabilities();
        let rx = b.subscribe();
        let handle = tokio::spawn(async move {
            let _ = handle_admin_connection(server_side, snapshot, capabilities, rx).await;
        });

        // Read the snapshot frame.
        use tokio::io::AsyncBufReadExt;
        let mut reader = tokio::io::BufReader::new(&mut client_side);
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).await.unwrap();
        assert!(n > 0);
        let v = parse_admin_frame(&line);
        assert_eq!(v["status"], "loading_model");
        assert_eq!(v["phase"], "checking_local");

        // Publish another event; client should see it.
        b.publish(StatusEvent::Ready);
        let mut line2 = Vec::new();
        let read =
            tokio::time::timeout(Duration::from_secs(1), reader.read_until(b'\n', &mut line2))
                .await
                .unwrap()
                .unwrap();
        assert!(read > 0);
        let v2 = parse_admin_frame(&line2);
        assert_eq!(v2["status"], "ready");

        drop(client_side);
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
}
