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

use inferd_daemon::lifecycle::{AcceptContext, DEFAULT_WRITE_TIMEOUT_SECS};
use inferd_daemon::lock::{Lock, LockError};
use inferd_daemon::redact::redact_in_place;
use inferd_proto::v2::{
    Attachment, ContentBlock, MAX_ATTACHMENTS_PER_REQUEST, MessageV2, RequestV2, RoleV2,
    WIRE_VERSION,
};
use inferd_proto::{MAX_FRAME_BYTES, ProtoError, read_lp_frame, write_lp_blob};
use std::io;
use std::time::Duration;

// =====================================================================
// F-1 / F-5: length-prefixed per-frame size cap (ADR 0021)
// =====================================================================

#[test]
fn f1_frame_cap_rejects_oversized_input() {
    use std::io::BufRead;

    // A length-prefixed frame whose declared payload_len exceeds
    // MAX_FRAME_BYTES. The bounded reader must refuse on the length
    // varint alone, before allocating or reading the payload — so a
    // reader that would yield garbage forever never gets that far.
    struct Endless {
        prefix: Vec<u8>,
        pos: usize,
    }
    impl io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // Serve the oversize length prefix first, then garbage —
            // the reader should bail before it ever reaches the garbage.
            if self.pos < self.prefix.len() {
                let n = (self.prefix.len() - self.pos).min(buf.len());
                buf[..n].copy_from_slice(&self.prefix[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            } else {
                buf.fill(b'a');
                Ok(buf.len())
            }
        }
    }
    impl BufRead for Endless {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.pos < self.prefix.len() {
                Ok(&self.prefix[self.pos..])
            } else {
                static CHUNK: [u8; 8192] = [b'a'; 8192];
                Ok(&CHUNK[..])
            }
        }
        fn consume(&mut self, n: usize) {
            self.pos += n;
        }
    }

    // LEB128-encode MAX_FRAME_BYTES + 1 as the payload length.
    let mut value = (MAX_FRAME_BYTES as u64) + 1;
    let mut prefix = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        prefix.push(byte);
        if value == 0 {
            break;
        }
    }

    let mut endless = Endless { prefix, pos: 0 };
    let err = read_lp_frame(&mut endless);
    assert!(matches!(err, Err(ProtoError::FrameTooLarge)));
}

#[test]
fn f1_frame_cap_rejects_oversized_output() {
    // A BLOB payload at the cap must be refused by the writer.
    let huge = vec![b'x'; MAX_FRAME_BYTES + 1];
    let mut buf = Vec::new();
    let err = write_lp_blob(&mut buf, &huge).unwrap_err();
    assert!(matches!(err, ProtoError::FrameTooLarge));
}

#[test]
fn f1_attachment_table_is_bounded_per_request() {
    // The per-frame cap bounds one frame; it does NOT bound one request.
    // Each declared attachment entitles the sender to one further BLOB
    // frame, so an unbounded attachment table multiplies a single in-cap
    // request frame into unbounded reads. The count cap closes that,
    // and the daemon's reader enforces it (plus a total-byte budget,
    // charged against the *declared* descriptor length) before any
    // payload is read — see `lifecycle_v2::read_attachment_blobs`.
    // Asserted here at the shared proto contract, which is the layer
    // every producer and every non-streaming consumer also sees.
    let over = MAX_ATTACHMENTS_PER_REQUEST + 1;
    let req = RequestV2 {
        wire_version: WIRE_VERSION,
        id: "amplify".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        attachments: (0..over)
            .map(|i| Attachment::Image {
                id: format!("img-{i}"),
                width: 1,
                height: 1,
                bytes: Vec::new(),
            })
            .collect(),
        ..Default::default()
    };
    let err = req.resolve().unwrap_err();
    match err {
        ProtoError::InvalidRequest(msg) => assert!(
            msg.contains("attachments"),
            "expected an attachment-cap rejection, got: {msg}"
        ),
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
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
// F-17: response writes are bounded by default
// =====================================================================

#[test]
fn f17_default_accept_policy_bounds_response_writes() {
    // The wedge itself — a non-reading peer holding an admission permit
    // through a blocked `write_all` — is reproduced over real sockets in
    // `tests/write_stall.rs`. What that test cannot catch is a
    // *regression of the default*: it injects a short bound so the
    // timeout is observable in test time, so swapping the production
    // default back to `None` (e.g. by deriving `Default` on
    // `AcceptContext`, where `Option::default()` is `None`) would leave
    // it green while every real daemon ran unbounded again.
    //
    // So assert the policy directly: whatever an `AcceptContext` is
    // constructed from, out of the box the write is bounded.
    let ctx = AcceptContext::default();
    assert_eq!(
        ctx.write_timeout,
        Some(Duration::from_secs(DEFAULT_WRITE_TIMEOUT_SECS)),
        "the default accept policy must bound response writes: an \
         unbounded write downstream of the admission gate lets a peer \
         that stops reading hold a generation slot forever"
    );
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
// F-7 / F-8: inbound TCP + its shared-key auth were REMOVED in v0.5.0
// (ADR 0022). The daemon binds no inbound network listener, so there is
// no un-attested TCP peer identity and no `auth.rs` shared-key compare
// to guard — the threat is closed by removal, not mitigation. The
// surviving transports (UDS / named pipe) are authenticated by kernel
// peer credentials, exercised by the lifecycle integration tests
// (peercred::unix / windows). The former `f7_*` / `f8_*` cases were
// deleted with the code they tested.
// =====================================================================
// F-1 corollary: unknown fields are tolerated on parse (forward compat
// per ADR 0015/0021; not strictly a security finding but adjacent —
// tests here keep the additive-evolution guarantee from rotting).
// =====================================================================

#[test]
fn additive_evolution_unknown_fields_are_ignored_on_parse() {
    // A v2 request JSON carrying a field this build doesn't know about
    // must still parse — the door for backwards-additive evolution
    // (ADR 0021) stays open only if unknown fields are ignored.
    let json = br#"{"wire_version":1,"id":"x","messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}],"future_field":42}"#;
    let req: RequestV2 = serde_json::from_slice(json).unwrap();
    assert_eq!(req.id, "x");
    assert_eq!(req.wire_version, WIRE_VERSION);
}

/// A round-tripped v2 request keeps its shape through serialise/parse.
/// Keeps `RequestV2` construction exercised in the security suite so the
/// wire-shape regression surface stays meaningful post-v1-excision.
#[test]
fn v2_request_round_trips_through_json() {
    let req = RequestV2 {
        wire_version: WIRE_VERSION,
        id: "rt".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }],
        ..Default::default()
    };
    let bytes = serde_json::to_vec(&req).unwrap();
    let back: RequestV2 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back, req);
}
