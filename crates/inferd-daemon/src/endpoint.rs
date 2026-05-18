//! IPC listener abstractions for inferd-daemon.
//!
//! v0.1 ships:
//! - **Unix domain socket** (Unix only) — the default inference transport.
//! - **Loopback TCP** — opt-in fallback for container / WSL scenarios; the
//!   default port is `127.0.0.1:47321`.
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
pub const DEFAULT_TCP_ADDR: &str = "127.0.0.1:47321";

/// Default admin endpoint per platform, per `docs/protocol-v1.md`
/// §"Admin endpoint".
///
/// On Linux the resolution chain is:
/// 1. `$XDG_RUNTIME_DIR/inferd/admin.sock` (set by `systemd-logind`
///    on session start; the per-user equivalent of `/run/<svc>/`).
/// 2. `$HOME/.inferd/run/admin.sock` for sessions without logind
///    (containers, ssh without a real login session).
/// 3. `/tmp/inferd-<uid>/admin.sock` as a last resort.
///
/// Historically inferd defaulted to `/run/inferd/admin.sock`. That
/// path is only writable by root and so was incompatible with
/// `systemd --user` units; the chain above matches the path the
/// systemd unit declares via `RuntimeDirectory=inferd`.
pub fn default_admin_addr() -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        linux_runtime_path("admin.sock")
    }
    #[cfg(target_os = "macos")]
    {
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push("admin.sock");
        p
    }
    #[cfg(windows)]
    {
        std::path::PathBuf::from(DEFAULT_ADMIN_PIPE_PATH)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        std::path::PathBuf::from("/tmp/inferd/admin.sock")
    }
}

/// Resolve a Linux runtime-dir path with the fallback chain
/// documented on `default_admin_addr`. `leaf` is the basename to
/// append (e.g. `admin.sock`, `infer.sock`, `inferd.lock`).
#[cfg(target_os = "linux")]
pub fn linux_runtime_path(leaf: &str) -> std::path::PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let mut p = std::path::PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            p.push("inferd");
            p.push(leaf);
            return p;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = std::path::PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            p.push(".inferd");
            p.push("run");
            p.push(leaf);
            return p;
        }
    }
    // Last resort: `/tmp/inferd-<uid>/<leaf>` — `<uid>` keeps
    // multi-user hosts from colliding on a shared /tmp.
    let uid = nix::unistd::Uid::current().as_raw();
    std::path::PathBuf::from(format!("/tmp/inferd-{uid}/{leaf}"))
}

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

/// Bind the *admin* Unix domain socket at `path` with mode `0600`
/// (Unix only). Stricter than `bind_uds`: the admin socket is
/// daemon-uid only — no group-shared inference, no operator group.
/// Per ADR 0009 + `docs/protocol-v1.md` §"Admin endpoint".
#[cfg(unix)]
pub async fn bind_admin_uds(path: &Path) -> io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("admin uds path is a symlink (refused): {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(listener)
}

/// Non-Unix stub for `bind_admin_uds`. Use [`bind_admin_pipe`] on
/// Windows.
#[cfg(not(unix))]
pub async fn bind_admin_uds(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix domain sockets are not supported on this platform; use bind_admin_pipe",
    ))
}

/// Bind the *admin* Windows named pipe at `path` (Windows only). The
/// default DACL applies (creating user only); per the inference-pipe
/// rationale, that's adequate for the per-user daemon model.
#[cfg(windows)]
pub fn bind_admin_pipe(
    path: &str,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first);
    opts.create(path)
}

/// Stub for non-Unix platforms; always returns `Unsupported`. On Windows,
/// callers should use [`bind_named_pipe`] instead.
#[cfg(not(unix))]
pub async fn bind_uds(_path: &Path, _group: Option<&str>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix domain sockets are not supported on this platform; use bind_named_pipe or TCP",
    ))
}

/// Default Windows named-pipe path for the inference endpoint.
#[cfg(windows)]
pub const DEFAULT_PIPE_PATH: &str = r"\\.\pipe\inferd-infer";

/// Default Windows named-pipe path for the admin endpoint per
/// `docs/protocol-v1.md` §"Admin endpoint".
#[cfg(windows)]
pub const DEFAULT_ADMIN_PIPE_PATH: &str = r"\\.\pipe\inferd-admin";

/// Bind a Windows named-pipe **server endpoint** at `path`.
///
/// Returns a single connected `NamedPipeServer` per accept; the caller
/// is expected to call `bind_named_pipe` again to open the next instance
/// (the standard Windows multi-instance pattern). `lifecycle::serve_named_pipe`
/// owns that loop.
///
/// **Security posture (THREAT_MODEL F-7, F-8):** v0.1 relies on the
/// default DACL applied by `CreateNamedPipe` when no security
/// attributes are passed — the creating user gets `GENERIC_ALL`,
/// `Everyone`/`Anonymous` are denied. That is *adequate* for the
/// per-user daemon model but not *sufficient* for the documented
/// "current SID only" target. SDDL hardening (DACL constructed from
/// the daemon's own SID, deny-all-others) is tracked as a v0.2
/// follow-up alongside `GetNamedPipeClientProcessId` for caller
/// identity. Documented in `THREAT_MODEL.md` F-7.
///
/// `first` controls whether the returned server is the very first
/// instance for `path` (which sets `FILE_FLAG_FIRST_PIPE_INSTANCE` to
/// reject if another process is already serving the same name). The
/// accept loop calls `bind_named_pipe(path, false)` for subsequent
/// instances.
#[cfg(windows)]
pub fn bind_named_pipe(
    path: &str,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first);
    opts.create(path)
}

#[cfg(windows)]
impl Connection for tokio::net::windows::named_pipe::NamedPipeServer {
    fn transport(&self) -> &'static str {
        "pipe"
    }
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

    #[cfg(windows)]
    #[tokio::test]
    async fn bind_named_pipe_accepts_a_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::windows::named_pipe::ClientOptions;

        // Use a unique pipe name per test invocation. Process-wide atomic
        // counter handles parallel tests within the same binary;
        // PID + timestamp ns spread across independent processes.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = format!(r"\\.\pipe\inferd-endpoint-test-{pid}-{ts}-{n}");

        let server = bind_named_pipe(&path, true).expect("bind named pipe");

        let path_for_server = path.clone();
        let server_task = tokio::spawn(async move {
            server.connect().await.expect("server connect");
            let mut s = server;
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            s.write_all(b"pong").await.unwrap();
            drop(path_for_server);
        });

        let mut client = ClientOptions::new()
            .open(&path)
            .expect("client open named pipe");
        client.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        server_task.await.unwrap();
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
