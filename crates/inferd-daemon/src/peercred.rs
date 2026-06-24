//! Peer-credential extraction for IPC connections.
//!
//! Implements [THREAT_MODEL.md F-7]: every accepted connection records
//! a `PeerIdentity` so the activity log can attribute every request to
//! a kernel-attested caller. ADR 0009 promises this.
//!
//! Transport posture:
//!
//! - **Unix domain socket**: kernel-attested via `SO_PEERCRED` on
//!   Linux, `LOCAL_PEERCRED` on macOS / BSD. Returns uid/gid/pid.
//! - **Windows named pipe**: `GetNamedPipeClientProcessId` plus an
//!   `OpenProcessToken`-based SID lookup. Returns sid/pid.
//! - **Loopback TCP**: not kernel-attested. The transport reduces to
//!   "remote address" — enough for log correlation only. F-8 covers
//!   the API-key auth that gives TCP its real perimeter.

use std::fmt;

/// Identity of the process on the other end of an IPC connection.
///
/// Field set varies by transport; `transport` names the source so
/// operators can tell kernel-enforced from self-reported. `Display`
/// produces the canonical log format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    /// Effective uid on Unix; `None` elsewhere.
    pub uid: Option<u32>,
    /// Effective gid on Unix; `None` elsewhere.
    pub gid: Option<u32>,
    /// Process id on Unix and Windows; `None` for TCP.
    pub pid: Option<u32>,
    /// User SID on Windows; `None` elsewhere.
    pub sid: Option<String>,
    /// Stable transport name: `"unix"` / `"pipe"`.
    pub transport: &'static str,
}

impl fmt::Display for PeerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.transport {
            "unix" => write!(
                f,
                "unix:{}:{}",
                self.uid.map(|u| u as i64).unwrap_or(-1),
                self.pid.map(|p| p as i64).unwrap_or(-1),
            ),
            "pipe" => write!(
                f,
                "pipe:{}:{}",
                self.sid.as_deref().unwrap_or("?"),
                self.pid.map(|p| p as i64).unwrap_or(-1),
            ),
            other => write!(f, "{other}:?"),
        }
    }
}

#[cfg(unix)]
pub mod unix {
    //! Unix-side peer credentials via the `nix` crate.
    //!
    //! Linux: single `SO_PEERCRED` getsockopt returns uid/gid/pid together.
    //! macOS: `LOCAL_PEERCRED` returns uid/gid (XuCred), `LOCAL_PEERPID` is
    //! a separate call — nix exposes these as `LocalPeerCred`/`LocalPeerPid`.

    #![allow(unsafe_code)] // BorrowedFd::borrow_raw is unsafe; scope tight.

    use super::PeerIdentity;
    use std::io;
    use std::os::fd::AsRawFd;
    use tokio::net::UnixStream;

    /// Extract peer credentials from a connected `UnixStream`.
    pub fn from_stream(stream: &UnixStream) -> io::Result<PeerIdentity> {
        let raw = stream.as_raw_fd();
        let borrowed = || unsafe { std::os::fd::BorrowedFd::borrow_raw(raw) };

        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
            let cred = getsockopt(&borrowed(), PeerCredentials)
                .map_err(|e| io::Error::other(format!("SO_PEERCRED: {e}")))?;
            Ok(PeerIdentity {
                uid: Some(cred.uid()),
                gid: Some(cred.gid()),
                pid: Some(cred.pid() as u32),
                sid: None,
                transport: "unix",
            })
        }

        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            use nix::sys::socket::{
                getsockopt,
                sockopt::{LocalPeerCred, LocalPeerPid},
            };
            let cred = getsockopt(&borrowed(), LocalPeerCred)
                .map_err(|e| io::Error::other(format!("LOCAL_PEERCRED: {e}")))?;
            let pid = getsockopt(&borrowed(), LocalPeerPid)
                .map_err(|e| io::Error::other(format!("LOCAL_PEERPID: {e}")))?;
            let gid = cred.groups().first().copied().unwrap_or(0);
            Ok(PeerIdentity {
                uid: Some(cred.uid()),
                gid: Some(gid),
                pid: Some(pid as u32),
                sid: None,
                transport: "unix",
            })
        }

        #[cfg(not(any(
            target_os = "android",
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
        )))]
        {
            let _ = borrowed;
            Err(io::Error::other(
                "SO_PEERCRED not supported on this platform",
            ))
        }
    }
}

#[cfg(windows)]
pub mod windows {
    //! Windows-side peer credentials via `GetNamedPipeClientProcessId` +
    //! `OpenProcessToken` for the SID.

    #![allow(unsafe_code)] // Win32 FFI surface; scoped to this module.

    use super::PeerIdentity;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Extract peer credentials from a connected `NamedPipeServer`.
    ///
    /// Uses `GetNamedPipeClientProcessId` to find the client PID, then
    /// `OpenProcessToken` + `GetTokenInformation(TokenUser)` to read
    /// the user SID. Both are read-only operations; the daemon needs
    /// no special privilege to call them on a peer in its own session.
    pub fn from_stream(server: &NamedPipeServer) -> io::Result<PeerIdentity> {
        let pipe_handle = server.as_raw_handle() as HANDLE;

        // SAFETY: pipe_handle is a connected named-pipe handle owned
        // by the NamedPipeServer; the call writes only to our local
        // `pid` variable.
        let mut pid: u32 = 0;
        let ok = unsafe { GetNamedPipeClientProcessId(pipe_handle, &mut pid) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }

        let sid = sid_for_pid(pid).unwrap_or(None);
        Ok(PeerIdentity {
            uid: None,
            gid: None,
            pid: Some(pid),
            sid,
            transport: "pipe",
        })
    }

    fn sid_for_pid(pid: u32) -> io::Result<Option<String>> {
        // SAFETY: every Win32 call below is documented to either
        // succeed and write the named out-parameter, or fail with a
        // GetLastError surface that we propagate. Handles are closed
        // via CloseHandle in the cleanup path.
        unsafe {
            let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
            if process == 0 {
                // Caller process may already be gone; treat as
                // best-effort and return None rather than failing the
                // whole accept.
                return Ok(None);
            }
            let mut token: HANDLE = 0;
            let ok = OpenProcessToken(process, TOKEN_QUERY, &mut token);
            if ok == 0 {
                CloseHandle(process);
                return Ok(None);
            }

            let mut needed: u32 = 0;
            // First call probes the required size.
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                CloseHandle(token);
                CloseHandle(process);
                return Ok(None);
            }

            let mut buf = vec![0u8; needed as usize];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr().cast(),
                needed,
                &mut needed,
            );
            if ok == 0 {
                CloseHandle(token);
                CloseHandle(process);
                return Ok(None);
            }

            let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
            let sid_ptr = token_user.User.Sid;

            let mut sid_str_ptr: *mut u16 = std::ptr::null_mut();
            let ok = ConvertSidToStringSidW(sid_ptr, &mut sid_str_ptr);
            CloseHandle(token);
            CloseHandle(process);
            if ok == 0 || sid_str_ptr.is_null() {
                return Ok(None);
            }

            // Walk the wide string until NUL.
            let mut len = 0usize;
            while *sid_str_ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(sid_str_ptr, len);
            let sid_string = String::from_utf16_lossy(slice);

            // Free the buffer ConvertSidToStringSidW allocated.
            LocalFree(sid_str_ptr as HLOCAL);

            Ok(Some(sid_string))
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_peer_credentials_self() {
        // Connect to a self-pair and assert we can read peer creds back
        // matching our own process.
        use tempfile::tempdir;
        use tokio::net::UnixListener;
        let dir = tempdir().unwrap();
        let path = dir.path().join("peer.sock");
        let listener = UnixListener::bind(&path).unwrap();

        let server = tokio::spawn(async move {
            let (server_side, _) = listener.accept().await.unwrap();
            unix::from_stream(&server_side).unwrap()
        });

        let _client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let id = server.await.unwrap();

        assert_eq!(id.transport, "unix");
        // pid is the same process — both ends of a self-connect run in
        // this test binary.
        assert_eq!(id.pid, Some(std::process::id()));
    }
}
