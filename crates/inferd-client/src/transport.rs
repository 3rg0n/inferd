//! Connection plumbing shared by every surface client.
//!
//! The generation, embed and rerank clients differ in framing and in the
//! frames they exchange; how they reach the daemon does not. Opening the
//! UDS / named pipe, splitting it, buffering the read half, and the
//! platform default-path chain live here once, so a transport change
//! (pipe open options, the Linux fallback chain) is made in one place.

use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

/// Read-side buffer every client wraps its connection in.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Boxed read half of a connection.
pub(crate) type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
/// Boxed write half of a connection.
pub(crate) type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

/// One open connection to a daemon socket: the write half, and the read
/// half behind a buffer.
pub(crate) struct Transport {
    pub(crate) write: BoxWrite,
    pub(crate) read: BufReader<BoxRead>,
}

impl Transport {
    /// Wrap already-open halves. Also the seam tests use to stub the
    /// transport with `tokio::io::duplex`.
    pub(crate) fn new(read: BoxRead, write: BoxWrite) -> Self {
        Self {
            write,
            read: BufReader::with_capacity(READ_BUFFER_BYTES, read),
        }
    }

    /// Open a Unix domain socket connection.
    #[cfg(unix)]
    pub(crate) async fn dial_uds(path: &std::path::Path) -> std::io::Result<Self> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        Ok(Self::new(Box::new(read), Box::new(write)))
    }

    /// Open a Windows named pipe connection.
    #[cfg(windows)]
    pub(crate) async fn dial_pipe(path: &str) -> std::io::Result<Self> {
        use tokio::net::windows::named_pipe::ClientOptions;
        let pipe = ClientOptions::new().open(path)?;
        let (read, write) = tokio::io::split(pipe);
        Ok(Self::new(Box::new(read), Box::new(write)))
    }
}

/// Default endpoint for one surface, mirroring the daemon's
/// `endpoint::default_*_addr` family.
///
/// `socket_file` is the Unix socket's file name (e.g. `"inferd.sock"`);
/// `pipe` is the full Windows pipe path (e.g. `r"\\.\pipe\inferd"`).
///
/// Linux fallback chain, shared by every surface:
/// 1. `${XDG_RUNTIME_DIR}/inferd/<socket_file>`
/// 2. `${HOME}/.inferd/run/<socket_file>`
/// 3. `/tmp/inferd/<socket_file>`
///
/// macOS uses `${TMPDIR}/inferd/<socket_file>`; other Unixes fall back to
/// `/tmp/inferd/<socket_file>`.
pub(crate) fn default_endpoint(socket_file: &str, pipe: &str) -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let _ = pipe;
        if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
            let mut p = PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                p.push("inferd");
                p.push(socket_file);
                return p;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = PathBuf::from(home);
            if !p.as_os_str().is_empty() {
                p.push(".inferd");
                p.push("run");
                p.push(socket_file);
                return p;
            }
        }
        PathBuf::from("/tmp/inferd").join(socket_file)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = pipe;
        let mut p = std::env::temp_dir();
        p.push("inferd");
        p.push(socket_file);
        p
    }
    #[cfg(windows)]
    {
        let _ = socket_file;
        PathBuf::from(pipe)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = pipe;
        PathBuf::from("/tmp/inferd").join(socket_file)
    }
}
