//! Windows named-pipe security descriptor construction.
//!
//! THREAT_MODEL F-7 promised a DACL on the inference + admin pipes
//! that grants only the daemon's own user SID. The default
//! `CreateNamedPipe` posture (no SECURITY_ATTRIBUTES passed) gives
//! the creating user `GENERIC_ALL` and denies `Everyone`/`Anonymous`,
//! which is *adequate* for a per-user daemon but not the
//! kernel-attested "current SID only" target the threat model
//! commits to.
//!
//! This module:
//! 1. Reads the current process's user SID at bind time.
//! 2. Builds an SDDL string of the form
//!    `O:<sid>G:<sid>D:P(A;;GA;;;<sid>)`:
//!    - `O:`/`G:` set owner/group to our SID.
//!    - `D:` is the DACL.
//!    - `P` makes the DACL protected — no inheritance from the
//!      parent object would grant anyone else access.
//!    - `(A;;GA;;;<sid>)` is one ACE: Allow / no flags / GENERIC_ALL
//!      / no object guid / no inherited object guid / our SID.
//!
//!    Anyone not our SID is implicitly denied because there is no
//!    ACE granting them access in a protected DACL.
//! 3. Converts the SDDL via
//!    `ConvertStringSecurityDescriptorToSecurityDescriptorW` into a
//!    heap-allocated `PSECURITY_DESCRIPTOR`.
//! 4. Wraps it in `SECURITY_ATTRIBUTES`.
//!
//! The caller passes the resulting `SECURITY_ATTRIBUTES` to
//! `tokio::net::windows::named_pipe::ServerOptions::
//! create_with_security_attributes_raw`. Windows copies the
//! descriptor into the kernel object on `CreateNamedPipe`, so the
//! caller's heap allocation can be freed after the call returns.
//! The `PipeSecurityDescriptor` RAII type below handles that.

#![allow(unsafe_code)] // Win32 FFI surface; scoped to this module.

use std::io;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::core::PWSTR;

/// Owns a heap-allocated security descriptor produced by
/// `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
///
/// The descriptor is allocated by Windows via `LocalAlloc`; we free
/// it via `LocalFree` on drop. The contained `SECURITY_ATTRIBUTES`
/// references the descriptor pointer, so this type also keeps the
/// descriptor alive for the lifetime of the attributes pointer
/// the caller passes to `CreateNamedPipe` /
/// `create_with_security_attributes_raw`.
///
/// Layout reminder: `SECURITY_ATTRIBUTES` is a small POD struct;
/// holding it inline (rather than boxed) is fine because we hand
/// out a `*mut SECURITY_ATTRIBUTES` pointing into our own storage,
/// and we keep `Self` alive for as long as that pointer is in use.
pub struct PipeSecurityDescriptor {
    /// Raw pointer returned by Windows; freed in `Drop`.
    descriptor: PSECURITY_DESCRIPTOR,
    /// Lives inline so the address is stable for the caller.
    attrs: SECURITY_ATTRIBUTES,
}

impl PipeSecurityDescriptor {
    /// Build a security descriptor that grants `GENERIC_ALL` to the
    /// current process's user SID and nobody else (protected DACL).
    pub fn current_user_only() -> io::Result<Self> {
        let sid_str = current_user_sid_string()?;
        // `O:S-1-5-21-...G:S-1-5-21-...D:P(A;;GA;;;S-1-5-21-...)`
        let sddl: Vec<u16> = format!("O:{sid_str}G:{sid_str}D:P(A;;GA;;;{sid_str})")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: sddl is null-terminated UTF-16. The `&mut descriptor`
        // out-pointer is a stack local. On success Windows writes a
        // heap pointer into it that we own and `LocalFree` in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }

        let attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: FALSE,
        };

        Ok(Self { descriptor, attrs })
    }

    /// Raw `*mut SECURITY_ATTRIBUTES` for tokio's
    /// `create_with_security_attributes_raw`. Pointer is stable for
    /// the lifetime of `self`.
    pub fn as_attrs_ptr(&mut self) -> *mut std::ffi::c_void {
        (&mut self.attrs) as *mut SECURITY_ATTRIBUTES as *mut std::ffi::c_void
    }
}

impl Drop for PipeSecurityDescriptor {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: descriptor was returned by
            // ConvertStringSecurityDescriptorToSecurityDescriptorW,
            // which documents `LocalFree` as the release path. Once
            // freed we null the pointer to make double-drop a
            // (silent) no-op.
            unsafe {
                LocalFree(self.descriptor as HLOCAL);
            }
            self.descriptor = ptr::null_mut();
        }
    }
}

/// Read the current process's user SID and return it as the
/// `S-1-5-21-...` string form SDDL expects.
fn current_user_sid_string() -> io::Result<String> {
    // SAFETY: every Win32 call below is documented to either succeed
    // and write the named out-parameter, or fail with a GetLastError
    // surface that we propagate. Handles are closed via CloseHandle
    // in the cleanup path.
    unsafe {
        let process: HANDLE = GetCurrentProcess();
        let mut token: HANDLE = 0;
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }

        // Probe required buffer size.
        let mut needed: u32 = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            CloseHandle(token);
            return Err(io::Error::other("GetTokenInformation: zero-size response"));
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
            let err = io::Error::last_os_error();
            CloseHandle(token);
            return Err(err);
        }

        let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid_ptr = token_user.User.Sid;

        let mut sid_str_ptr: PWSTR = ptr::null_mut();
        let ok = ConvertSidToStringSidW(sid_ptr, &mut sid_str_ptr);
        CloseHandle(token);
        if ok == 0 || sid_str_ptr.is_null() {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: ConvertSidToStringSidW returns a null-terminated
        // UTF-16 string we own; LocalFree is the release path.
        let s = wstr_to_string(sid_str_ptr);
        LocalFree(sid_str_ptr as HLOCAL);
        s
    }
}

/// Read a null-terminated UTF-16 buffer into an owned `String`.
unsafe fn wstr_to_string(ptr: PWSTR) -> io::Result<String> {
    if ptr.is_null() {
        return Err(io::Error::other("null SID string"));
    }
    let mut len = 0usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16(slice)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("UTF-16: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_user_sid_is_well_formed() {
        let s = current_user_sid_string().unwrap();
        assert!(
            s.starts_with("S-1-"),
            "SID should start with S-1- prefix, got {s:?}"
        );
        // SDDL grammar requires the form `S-1-<authority>-<sub>...`,
        // at minimum 3 hyphen-separated components.
        assert!(
            s.matches('-').count() >= 2,
            "SID has too few components: {s:?}"
        );
    }

    #[test]
    fn pipe_security_descriptor_constructs() {
        // Just exercising the happy path; deeper validation requires
        // creating a real pipe, which the integration test below does.
        let _sd = PipeSecurityDescriptor::current_user_only().unwrap();
    }

    #[tokio::test]
    async fn pipe_with_hardened_dacl_accepts_self() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

        // Use a unique-per-pid pipe name so parallel tests don't
        // collide on a system pipe namespace.
        let path = format!(r"\\.\pipe\inferd-sddl-test-{}", std::process::id());

        let mut sd = PipeSecurityDescriptor::current_user_only().unwrap();
        // SAFETY: as_attrs_ptr returns a stable pointer for the
        // lifetime of `sd`; tokio reads it during `create` and
        // Windows copies the descriptor into the kernel object.
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create_with_security_attributes_raw(&path, sd.as_attrs_ptr())
        }
        .expect("bind hardened pipe");

        // The current user IS the SID we put in the DACL, so connecting
        // as ourselves must succeed.
        let client_path = path.clone();
        let client = tokio::spawn(async move {
            // Brief retry loop in case the listener side hasn't yet
            // entered `connect`.
            for _ in 0..10 {
                match ClientOptions::new().open(&client_path) {
                    Ok(c) => return c,
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
                }
            }
            panic!("client failed to connect to hardened pipe");
        });

        server.connect().await.expect("server.connect");
        let mut client = client.await.unwrap();

        // Round-trip a byte to prove the connection works.
        client.write_all(b"x").await.unwrap();
        let mut buf = [0u8; 1];
        let mut server_io = server;
        let n = server_io.read(&mut buf).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(&buf, b"x");
    }
}
