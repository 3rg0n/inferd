//! IPC listener abstractions for inferd-daemon.
//!
//! v0.1 ships:
//! - **Unix domain socket** (Unix only) — the default inference transport.
//! - **Loopback TCP** — opt-in fallback for container / WSL scenarios; the
//!   default port is `127.0.0.1:47321` (one above thlibo's historical
//!   47320 to allow side-by-side operation).
//!
//! Windows named pipe support is deferred to M4. This is fine for the M1
//! exit criterion, which uses TCP for cross-platform integration testing.
//!
//! ## Ready gating (THREAT_MODEL F-13)
//!
//! Listeners are created by `bind_*` functions only — they never bind in the
//! constructor. The lifecycle calls `bind_*` *after* the configured backend
//! reports `ready()`, so the OS-level socket simply does not exist until
//! the daemon is willing to accept work.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};

/// Default loopback port for the optional TCP transport.
///
/// Distinct from thlibo's historical `47320` so an inferd instance can run
/// alongside an old thlibod during migration without port conflicts.
pub const DEFAULT_TCP_ADDR: &str = "127.0.0.1:47321";

/// Trait abstracting an accepted connection so the lifecycle can speak to
/// either a Unix-socket stream or a TCP stream uniformly.
pub trait Connection: AsyncRead + AsyncWrite + Unpin + Send {
    /// Stable string identifying the transport ("unix"/"tcp"). Used for
    /// activity-log attribution; not echoed on the wire.
    fn transport(&self) -> &'static str;
}

impl Connection for TcpStream {
    fn transport(&self) -> &'static str {
        "tcp"
    }
}

#[cfg(unix)]
impl Connection for tokio::net::UnixStream {
    fn transport(&self) -> &'static str {
        "unix"
    }
}

/// Bind a loopback TCP listener at `addr`.
///
/// `addr` must parse as a `SocketAddr`. By convention the daemon binds
/// `127.0.0.1` only — operators wanting a different bind have to opt in
/// explicitly via configuration. We do not attempt to enforce loopback-only
/// here because that's a config-layer decision; the threat model documents
/// the consequence (F-8) when an operator chooses non-loopback.
pub async fn bind_tcp(addr: &str) -> io::Result<TcpListener> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("bad tcp addr: {e}")))?;
    TcpListener::bind(parsed).await
}

/// Bind a Unix domain socket at `path` with mode `0660` and the given group
/// (Unix only).
///
/// On Windows this returns `Err(Unsupported)` — UDS support is M4 (named
/// pipe).
#[cfg(unix)]
pub async fn bind_uds(path: &Path, group: Option<&str>) -> io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    // Remove a stale socket file from a previous run before binding. Stat
    // first to refuse if it's a symlink (hardening; F-2 is for the lock
    // path, but the same hygiene applies to listener paths).
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("uds path is a symlink (refused): {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o660);
    std::fs::set_permissions(path, perms)?;

    if let Some(group_name) = group {
        chown_to_group(path, group_name)?;
    }
    Ok(listener)
}

/// Stub for non-Unix platforms; always returns `Unsupported`.
///
/// Windows named-pipe support lands in M4. Until then, Windows operators
/// must use the loopback TCP transport.
#[cfg(not(unix))]
pub async fn bind_uds(_path: &Path, _group: Option<&str>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix domain sockets are not supported on this platform; use TCP or wait for M4 Windows pipe support",
    ))
}

#[cfg(unix)]
fn chown_to_group(path: &Path, group_name: &str) -> io::Result<()> {
    let group = nix::unistd::Group::from_name(group_name)
        .map_err(|e| io::Error::other(format!("getgrnam: {e}")))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("group not found: {group_name}"),
            )
        })?;
    nix::unistd::chown(path, None, Some(group.gid))
        .map_err(|e| io::Error::other(format!("chown: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn bind_tcp_accepts_a_connection() {
        let listener = bind_tcp("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            sock.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            sock.write_all(b"pong").await.unwrap();
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bind_tcp_rejects_garbage_addr() {
        let err = bind_tcp("not-an-addr").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bind_uds_creates_socket_and_accepts() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.sock");
        let listener = bind_uds(&path, None).await.unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            sock.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        });

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bind_uds_refuses_symlink_path() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let target = dir.path().join("real.sock");
        std::fs::write(&target, b"").unwrap();
        let symlink = dir.path().join("link.sock");
        std::os::unix::fs::symlink(&target, &symlink).unwrap();

        let err = bind_uds(&symlink, None).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
