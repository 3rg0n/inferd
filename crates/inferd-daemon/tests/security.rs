//! Tier 5 security regression suite per `docs/test-strategy.md`.
//!
//! Compiled in only under `--features security` so a developer can
//! run a coherent THREAT_MODEL regression sweep with one command:
//!
//! ```text
//! cargo test -p inferd-daemon --features security --test security
//! ```
//!
//! Each test is named `f<N>_<description>` and corresponds to one
//! THREAT_MODEL.md finding. The bodies assert the invariant directly
//! rather than calling out to module-internal tests; this keeps the
//! Tier 5 surface readable as a security-property checklist even
//! when the underlying mitigation lives behind a complex API.
//!
//! New findings → new test here, named consistently.

#![cfg(feature = "security")]

use inferd_daemon::auth::{key_matches, AuthFrame};
use inferd_daemon::lock::{Lock, LockError};
use inferd_daemon::peercred::PeerIdentity;
use inferd_daemon::redact::redact_in_place;
use inferd_proto::{read_frame, write_frame, Message, ProtoError, Request, Role, MAX_FRAME_BYTES};
use std::io;
use std::io::Cursor;

// =====================================================================
// F-1: NDJSON per-frame size cap
// =====================================================================

#[test]
fn f1_frame_cap_rejects_oversized_input() {
    use std::io::BufRead;

    // Reader that returns garbage bytes forever, no newline. The
    // bounded reader must refuse without exhausting heap.
    struct Endless;
    impl io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            buf.fill(b'a');
            Ok(buf.len())
        }
    }
    impl BufRead for Endless {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            static CHUNK: [u8; 8192] = [b'a'; 8192];
            Ok(&CHUNK[..])
        }
        fn consume(&mut self, _n: usize) {}
    }

    let mut endless = Endless;
    let err: Result<Option<Request>, _> = read_frame(&mut endless);
    assert!(matches!(err, Err(ProtoError::FrameTooLarge)));
}

#[test]
fn f1_frame_cap_rejects_oversized_output() {
    let huge = "x".repeat(MAX_FRAME_BYTES);
    let req = Request {
        id: "id".into(),
        messages: vec![Message {
            role: Role::User,
            content: huge,
        }],
        temperature: None,
        top_p: None,
        top_k: None,
        max_tokens: None,
        stream: None,
        image_token_budget: None,
        grammar: String::new(),
    };
    let mut buf = Vec::new();
    let err = write_frame(&mut buf, &req).unwrap_err();
    assert!(matches!(err, ProtoError::FrameTooLarge));
}

// =====================================================================
// F-2: lock-file pre-existing symlink rejection
// =====================================================================

#[test]
fn f2_lock_acquire_then_release_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("inferd.lock");
    let lock = Lock::acquire(&path).expect("acquire");
    drop(lock);
    let _again = Lock::acquire(&path).expect("re-acquire after drop");
}

#[test]
fn f2_lock_directory_at_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("inferd.lock");
    std::fs::create_dir(&bad).unwrap();
    let err = Lock::acquire(&bad).unwrap_err();
    assert!(matches!(err, LockError::NotARegularFile(_)));
}

#[cfg(unix)]
#[test]
fn f2_lock_pre_existing_symlink_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.bin");
    std::fs::write(&target, b"x").unwrap();
    let symlink = dir.path().join("inferd.lock");
    std::os::unix::fs::symlink(&target, &symlink).unwrap();

    let err = Lock::acquire(&symlink).unwrap_err();
    assert!(matches!(err, LockError::Symlink(_)));
}

// =====================================================================
// F-3: write-time secret redactor
// =====================================================================

#[test]
fn f3_redactor_scrubs_known_credential_shapes() {
    // Synthetic fixtures assembled at runtime so secret-scanning tools
    // don't flag the source.
    let sk = format!("{}-{}", "sk", "abcdefghijklmnopqrst");
    let aws = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
    let ghp = format!("{}_{}", "ghp", "abcdefghijklmnopqrstuvwxyz12");
    let mut record = format!(r#"{{"key":"{sk}","aws":"{aws}","gh":"{ghp}"}}"#);

    redact_in_place(&mut record);

    assert!(!record.contains(&sk), "sk- leaked: {record}");
    assert!(!record.contains(&aws), "aws AKIA leaked: {record}");
    assert!(!record.contains(&ghp), "ghp_ leaked: {record}");
    assert!(
        record.contains("[REDACTED"),
        "no redaction marker: {record}"
    );
}

#[test]
fn f3_redactor_passes_through_safe_text() {
    let mut s = "the quick brown fox jumps over the lazy dog".to_string();
    let original = s.clone();
    redact_in_place(&mut s);
    assert_eq!(s, original);
}

// =====================================================================
// F-7: PeerIdentity surface stable
// =====================================================================

#[test]
fn f7_tcp_identity_has_no_kernel_attestation() {
    let id = PeerIdentity::from_tcp("127.0.0.1:12345".parse().unwrap());
    assert_eq!(id.transport, "tcp");
    assert!(id.uid.is_none());
    assert!(id.gid.is_none());
    assert!(id.pid.is_none());
    assert!(id.sid.is_none());
    assert!(id.remote_addr.is_some());
}

// =====================================================================
// F-8: API key constant-time compare
// =====================================================================

#[test]
fn f8_key_matches_equal_keys() {
    assert!(key_matches("super-secret", "super-secret"));
}

#[test]
fn f8_key_matches_rejects_different_lengths() {
    assert!(!key_matches("short", "longer-secret-string"));
}

#[test]
fn f8_key_matches_rejects_subtle_difference() {
    assert!(!key_matches("Super-Secret", "super-secret"));
}

#[test]
fn f8_auth_frame_parses_correct_shape() {
    let frame = AuthFrame::from_json(br#"{"type":"auth","key":"hello"}"#).unwrap();
    assert_eq!(frame.key, "hello");
}

#[test]
fn f8_auth_frame_rejects_non_auth_type() {
    assert!(AuthFrame::from_json(br#"{"type":"request","messages":[]}"#).is_none());
}

// =====================================================================
// F-1 corollary: unknown fields are tolerated on parse (forward compat
// per ADR 0008; not strictly a security finding but adjacent — tests
// here keep the additive-evolution guarantee from rotting).
// =====================================================================

#[test]
fn additive_evolution_unknown_fields_are_ignored_on_parse() {
    let json = br#"{"id":"x","messages":[{"role":"user","content":"hi"}],"future_field":42}
"#;
    let mut cursor = Cursor::new(&json[..]);
    let req: Request = read_frame(&mut cursor).unwrap().unwrap();
    assert_eq!(req.id, "x");
}
