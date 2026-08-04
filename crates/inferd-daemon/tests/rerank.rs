//! Integration test for the rerank socket dispatch path (ADR 0027).
//!
//! Pins the contract that the rerank listener:
//!   - parses NDJSON `RerankRequest` frames and emits exactly one
//!     terminal `RerankResponse` frame per request;
//!   - returns results sorted by score **descending**, truncated to
//!     `top_n`, with `backend` + `model` + `usage` populated;
//!   - rejects `resolve()` failures with `invalid_request` and keeps the
//!     connection open (a bad request is not a bad connection);
//!   - emits `queue_full` when the shared admission gate is saturated —
//!     rerank shares one gate with generation and embed;
//!   - emits `backend_unavailable` when no registered backend advertises
//!     `capabilities().rerank`, and `rerank_unsupported` when one
//!     advertises the capability but the adapter can't serve it (the
//!     misconfiguration fail-safe);
//!   - closes the connection after a frame-level decode error.
//!
//! Unlike `v2_stub.rs` (UDS only) this file runs on both transports: the
//! harness at the top resolves to a Unix socket on Unix and a named pipe
//! on Windows, so every test body below is transport-agnostic. That
//! matters because the rerank surface's peercred + accept-loop code is
//! per-platform, and a Unix-only test would leave half of it unexercised.

#![allow(dead_code)]

use async_trait::async_trait;
use inferd_daemon::lifecycle::wait_for_ready;
use inferd_daemon::lifecycle_rerank::AcceptContext;
use inferd_daemon::queue::Admission;
use inferd_daemon::router::Router;
use inferd_engine::mock::{Mock, MockConfig, MockError};
use inferd_engine::{
    Backend, BackendCapabilities, GenerateError, RerankError, RerankOutcome, TokenStreamV2,
};
use inferd_proto::rerank::{
    MAX_RERANK_DOCUMENTS, RerankErrorCode, RerankRequest, RerankResolved, RerankResponse,
};
use inferd_proto::v2::ResolvedV2;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ---------------------------------------------------------------------------
// Transport harness
// ---------------------------------------------------------------------------

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// A booted rerank listener plus the handles needed to stop it.
struct Daemon {
    addr: String,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl Daemon {
    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), self.handle).await;
    }
}

/// Unique listener address per test, so the whole file can run in
/// parallel without cross-talk.
fn unique_addr(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!("inferd-test-rerank-{tag}-{pid}-{n}.sock"))
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(windows)]
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!(r"\\.\pipe\inferd-test-rerank-{tag}-{pid}-{ts}-{n}")
    }
}

/// Boot a rerank listener over the platform's IPC transport.
async fn boot(tag: &str, backends: Vec<Arc<dyn Backend>>, ctx: AcceptContext) -> Daemon {
    let router = Arc::new(Router::new(backends));
    // Not strictly required (the mock is ready on construction), but it
    // pins the same ordering the daemon binary uses: backends ready
    // before the socket exists (THREAT_MODEL F-13).
    let _ = wait_for_ready(&router, Duration::from_secs(2)).await;

    let addr = unique_addr(tag);
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();

    #[cfg(unix)]
    let handle = {
        let path = std::path::PathBuf::from(&addr);
        let _ = std::fs::remove_file(&path);
        let listener = inferd_daemon::endpoint::bind_uds(&path, None)
            .await
            .expect("bind uds");
        tokio::spawn(async move {
            let _ = inferd_daemon::lifecycle_rerank::serve_uds_rerank(
                listener,
                router,
                ctx,
                shutdown_rx,
            )
            .await;
        })
    };
    #[cfg(windows)]
    let handle = {
        // Pre-bind the first instance so the pipe exists before `boot`
        // returns; otherwise a client can connect between the spawn and
        // the first bind.
        let first = inferd_daemon::endpoint::bind_named_pipe(&addr, true).expect("bind first pipe");
        let path = addr.clone();
        tokio::spawn(async move {
            let _ = inferd_daemon::lifecycle_rerank::serve_named_pipe_rerank(
                &path,
                first,
                router,
                ctx,
                shutdown_rx,
            )
            .await;
        })
    };

    Daemon {
        addr,
        shutdown,
        handle,
    }
}

/// A connected rerank client: NDJSON out, one terminal frame back.
struct Conn {
    reader: BufReader<tokio::io::ReadHalf<Stream>>,
    writer: tokio::io::WriteHalf<Stream>,
}

impl Conn {
    async fn connect(addr: &str) -> Self {
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(addr)
            .await
            .expect("connect");
        #[cfg(windows)]
        let stream = {
            use tokio::net::windows::named_pipe::ClientOptions;
            // Windows named-pipe open can transiently fail with "all
            // instances are busy" while the server is between accept and
            // bind-next-instance.
            let mut opened = None;
            for attempt in 0..40 {
                match ClientOptions::new().open(addr) {
                    Ok(c) => {
                        opened = Some(c);
                        break;
                    }
                    Err(e) if attempt < 39 => {
                        let _ = e;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Err(e) => panic!("client open failed: {e}"),
                }
            }
            opened.expect("client connected")
        };
        let (read, writer) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(read),
            writer,
        }
    }

    async fn send(&mut self, req: &RerankRequest) {
        let mut buf = serde_json::to_vec(req).expect("serialise rerank request");
        buf.push(b'\n');
        self.writer.write_all(&buf).await.expect("write request");
        self.writer.flush().await.expect("flush request");
    }

    /// Write a raw NDJSON line, for the malformed-payload case.
    async fn send_raw(&mut self, line: &[u8]) {
        self.writer.write_all(line).await.expect("write raw");
        self.writer.write_all(b"\n").await.expect("write newline");
        self.writer.flush().await.expect("flush raw");
    }

    /// Read one terminal frame. Panics on EOF — a test that expected a
    /// frame should fail loudly rather than silently pass.
    async fn recv(&mut self) -> RerankResponse {
        self.try_recv()
            .await
            .expect("daemon closed without a frame")
    }

    /// Read one frame, or `None` on a clean EOF.
    async fn try_recv(&mut self) -> Option<RerankResponse> {
        let mut line = Vec::new();
        let n = tokio::time::timeout(
            Duration::from_secs(10),
            self.reader.read_until(b'\n', &mut line),
        )
        .await
        .expect("read budget exceeded — daemon hung?")
        .expect("read error");
        if n == 0 {
            return None;
        }
        Some(serde_json::from_slice(&line).expect("decode rerank response frame"))
    }

    async fn round_trip(&mut self, req: &RerankRequest) -> RerankResponse {
        self.send(req).await;
        self.recv().await
    }
}

// ---------------------------------------------------------------------------
// Test backends
// ---------------------------------------------------------------------------

/// A backend that serves generation but advertises no rerank capability —
/// the router must skip it, leaving the request with no candidate.
struct NoRerank;

#[async_trait]
impl Backend for NoRerank {
    fn name(&self) -> &str {
        "no-rerank"
    }
    fn ready(&self) -> bool {
        true
    }
    fn capabilities(&self) -> BackendCapabilities {
        // v2 only; `rerank` stays false.
        BackendCapabilities {
            v2: true,
            ..BackendCapabilities::default()
        }
    }
    async fn generate_v2(&self, _req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        Err(GenerateError::Internal("not used in this test".into()))
    }
}

/// A backend that *claims* rerank but leaves the trait's default
/// `rerank()` in place (which returns `Unsupported`). This is the
/// misconfiguration the daemon guards against: the socket should never
/// have been bound, and the request must fail as `rerank_unsupported`
/// rather than as a generic backend error.
struct ClaimsRerank;

#[async_trait]
impl Backend for ClaimsRerank {
    fn name(&self) -> &str {
        "claims-rerank"
    }
    fn ready(&self) -> bool {
        true
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            rerank: true,
            ..BackendCapabilities::default()
        }
    }
    async fn generate_v2(&self, _req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        Err(GenerateError::Internal("not used in this test".into()))
    }
    // No `rerank` override — the trait default yields
    // `RerankError::Unsupported`.
}

/// A backend whose `rerank()` fails outright, to exercise the
/// error-mapping arm the mock's knob doesn't reach.
struct InternalErrorRerank;

#[async_trait]
impl Backend for InternalErrorRerank {
    fn name(&self) -> &str {
        "internal-error-rerank"
    }
    fn ready(&self) -> bool {
        true
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            rerank: true,
            ..BackendCapabilities::default()
        }
    }
    async fn generate_v2(&self, _req: ResolvedV2) -> Result<TokenStreamV2, GenerateError> {
        Err(GenerateError::Internal("not used in this test".into()))
    }
    async fn rerank(&self, _req: RerankResolved) -> Result<RerankOutcome, RerankError> {
        Err(RerankError::Internal("scoring exploded".into()))
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn mock_backend() -> Arc<dyn Backend> {
    Arc::new(Mock::new())
}

/// A query plus three documents with strictly decreasing overlap against
/// it, so the mock's word-overlap scorer produces a total order the test
/// can assert exactly:
///   - index 0: no query words          → 0/4
///   - index 1: two query words         → 2/4
///   - index 2: all four query words    → 4/4
///
/// Expected ordering: [2, 1, 0].
fn graded_request(id: &str) -> RerankRequest {
    RerankRequest {
        id: id.into(),
        query: "bind unix domain socket".into(),
        documents: vec![
            "A recipe for cheese scones.".into(),
            "How to bind a TCP socket.".into(),
            "Call bind(2) to bind a unix domain socket.".into(),
        ],
        top_n: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn success_frame_is_sorted_descending_and_names_the_backend() {
    let d = boot("ok", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    let resp = conn.round_trip(&graded_request("rr-1")).await;
    match resp {
        RerankResponse::Rerank {
            id,
            results,
            model,
            usage,
            backend,
        } => {
            assert_eq!(id, "rr-1");
            assert_eq!(backend, "mock", "terminal frame must name the adapter");
            assert_eq!(model, "mock");
            assert!(
                usage.input_tokens > 0,
                "cross-encoder usage is per query/document pair, never zero"
            );
            assert_eq!(results.len(), 3, "no top_n → every document scored");
            assert_eq!(
                results.iter().map(|r| r.index).collect::<Vec<_>>(),
                vec![2, 1, 0],
                "results must arrive sorted by score descending"
            );
            for w in results.windows(2) {
                assert!(
                    w[0].score >= w[1].score,
                    "scores must be monotonically non-increasing: {results:?}"
                );
            }
        }
        other => panic!("expected Rerank, got {other:?}"),
    }

    d.stop().await;
}

#[tokio::test]
async fn top_n_truncates_to_the_highest_scoring_documents() {
    let d = boot("topn", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    let mut req = graded_request("rr-topn");
    req.top_n = Some(2);
    match conn.round_trip(&req).await {
        RerankResponse::Rerank { results, .. } => {
            assert_eq!(results.len(), 2, "top_n must truncate");
            assert_eq!(
                results.iter().map(|r| r.index).collect::<Vec<_>>(),
                vec![2, 1],
                "truncation must keep the top of the ordering, not the head of the input"
            );
        }
        other => panic!("expected Rerank, got {other:?}"),
    }

    d.stop().await;
}

/// `top_n` above the document count means "all of them" — a caller whose
/// candidate set shrank shouldn't have to clamp.
#[tokio::test]
async fn top_n_larger_than_documents_returns_all() {
    let d = boot("topn-big", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    let mut req = graded_request("rr-topn-big");
    req.top_n = Some(99);
    match conn.round_trip(&req).await {
        RerankResponse::Rerank { results, .. } => assert_eq!(results.len(), 3),
        other => panic!("expected Rerank, got {other:?}"),
    }

    d.stop().await;
}

/// A rejected request must not cost the caller its connection: the
/// daemon answers `invalid_request` and keeps reading. Batch clients that
/// pipeline requests over one long-lived connection depend on this.
#[tokio::test]
async fn invalid_request_is_rejected_and_the_connection_stays_open() {
    let d = boot("invalid", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    // Empty documents: rejected by `resolve()`.
    let resp = conn
        .round_trip(&RerankRequest {
            id: "rr-empty".into(),
            query: "q".into(),
            documents: vec![],
            top_n: None,
        })
        .await;
    match resp {
        RerankResponse::Error { id, code, message } => {
            assert_eq!(id, "rr-empty", "error frame must echo the request id");
            assert_eq!(code, RerankErrorCode::InvalidRequest);
            assert!(
                message.contains("documents"),
                "message should name the offending field; got: {message}"
            );
        }
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    // `top_n: 0` — an empty result set is never what a caller wants.
    let mut zero_top_n = graded_request("rr-zero");
    zero_top_n.top_n = Some(0);
    match conn.round_trip(&zero_top_n).await {
        RerankResponse::Error { id, code, .. } => {
            assert_eq!(id, "rr-zero");
            assert_eq!(code, RerankErrorCode::InvalidRequest);
        }
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    // Same connection still serves a good request.
    match conn.round_trip(&graded_request("rr-after")).await {
        RerankResponse::Rerank { id, .. } => assert_eq!(id, "rr-after"),
        other => panic!("expected Rerank after recovery, got {other:?}"),
    }

    d.stop().await;
}

/// The document-count cap is the F-1-class bound that keeps a cheap frame
/// from buying `O(documents)` forward passes. Pinned end-to-end, not just
/// in the proto unit tests, because the daemon is where the bound has to
/// fire — before the admission permit is held for the whole batch.
#[tokio::test]
async fn document_count_over_the_cap_is_rejected() {
    let d = boot("cap", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    let resp = conn
        .round_trip(&RerankRequest {
            id: "rr-cap".into(),
            query: "q".into(),
            documents: (0..MAX_RERANK_DOCUMENTS + 1)
                .map(|i| format!("doc {i}"))
                .collect(),
            top_n: None,
        })
        .await;
    match resp {
        RerankResponse::Error { code, message, .. } => {
            assert_eq!(code, RerankErrorCode::InvalidRequest);
            assert!(
                message.contains(&MAX_RERANK_DOCUMENTS.to_string()),
                "message should state the cap; got: {message}"
            );
        }
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }

    // Exactly at the cap is accepted.
    match conn
        .round_trip(&RerankRequest {
            id: "rr-at-cap".into(),
            query: "q".into(),
            documents: (0..MAX_RERANK_DOCUMENTS)
                .map(|i| format!("doc {i}"))
                .collect(),
            top_n: Some(1),
        })
        .await
    {
        RerankResponse::Rerank { results, .. } => assert_eq!(results.len(), 1),
        other => panic!("expected Rerank at the cap, got {other:?}"),
    }

    d.stop().await;
}

/// Admission is shared across every wire surface — one slot is one slot.
/// Holding the only permit from the test itself makes the overflow
/// deterministic instead of racing a slow backend.
#[tokio::test]
async fn queue_full_when_the_shared_admission_gate_is_saturated() {
    let admission = Admission::new(1, 0);
    let ctx = AcceptContext {
        admission: Some(admission.clone()),
        ..Default::default()
    };
    let d = boot("qfull", vec![mock_backend()], ctx).await;
    let mut conn = Conn::connect(&d.addr).await;

    // Occupy the only slot, as a concurrent generation would.
    let held = admission.try_admit().expect("first permit");
    assert_eq!(admission.available_permits(), 0);

    match conn.round_trip(&graded_request("rr-full")).await {
        RerankResponse::Error { id, code, .. } => {
            assert_eq!(id, "rr-full", "queue_full frame must echo the request id");
            assert_eq!(code, RerankErrorCode::QueueFull);
        }
        other => panic!("expected Error{{QueueFull,..}}, got {other:?}"),
    }

    // Releasing the slot makes the same connection usable again.
    drop(held);
    match conn.round_trip(&graded_request("rr-drained")).await {
        RerankResponse::Rerank { id, .. } => assert_eq!(id, "rr-drained"),
        other => panic!("expected Rerank once the slot freed, got {other:?}"),
    }

    d.stop().await;
}

/// No registered backend advertises `capabilities().rerank`, so the
/// router has no candidate. Production never binds the socket in this
/// configuration; the frame is the fail-safe.
#[tokio::test]
async fn no_rerank_capable_backend_yields_backend_unavailable() {
    let d = boot(
        "nocap",
        vec![Arc::new(NoRerank) as Arc<dyn Backend>],
        AcceptContext::default(),
    )
    .await;
    let mut conn = Conn::connect(&d.addr).await;

    match conn.round_trip(&graded_request("rr-nocap")).await {
        RerankResponse::Error { id, code, .. } => {
            assert_eq!(id, "rr-nocap");
            assert_eq!(code, RerankErrorCode::BackendUnavailable);
        }
        other => panic!("expected Error{{BackendUnavailable,..}}, got {other:?}"),
    }

    d.stop().await;
}

/// A backend that advertises the capability but can't serve it must map
/// to `rerank_unsupported`, distinctly from `backend_unavailable` — the
/// two point an operator at different misconfigurations.
#[tokio::test]
async fn capability_without_implementation_yields_rerank_unsupported() {
    let d = boot(
        "claims",
        vec![Arc::new(ClaimsRerank) as Arc<dyn Backend>],
        AcceptContext::default(),
    )
    .await;
    let mut conn = Conn::connect(&d.addr).await;

    match conn.round_trip(&graded_request("rr-claims")).await {
        RerankResponse::Error { id, code, .. } => {
            assert_eq!(id, "rr-claims");
            assert_eq!(code, RerankErrorCode::RerankUnsupported);
        }
        other => panic!("expected Error{{RerankUnsupported,..}}, got {other:?}"),
    }

    d.stop().await;
}

#[tokio::test]
async fn backend_failure_maps_to_backend_unavailable() {
    let mock = Arc::new(Mock::with_config(MockConfig {
        pre_stream_error: Some(MockError::Unavailable),
        ..Default::default()
    })) as Arc<dyn Backend>;
    let d = boot("unavail", vec![mock], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    match conn.round_trip(&graded_request("rr-unavail")).await {
        RerankResponse::Error { id, code, .. } => {
            assert_eq!(id, "rr-unavail");
            assert_eq!(code, RerankErrorCode::BackendUnavailable);
        }
        other => panic!("expected Error{{BackendUnavailable,..}}, got {other:?}"),
    }

    d.stop().await;
}

#[tokio::test]
async fn backend_internal_error_maps_to_internal() {
    let d = boot(
        "internal",
        vec![Arc::new(InternalErrorRerank) as Arc<dyn Backend>],
        AcceptContext::default(),
    )
    .await;
    let mut conn = Conn::connect(&d.addr).await;

    match conn.round_trip(&graded_request("rr-internal")).await {
        RerankResponse::Error {
            id,
            code,
            ref message,
        } => {
            assert_eq!(id, "rr-internal");
            assert_eq!(code, RerankErrorCode::Internal);
            assert!(
                message.contains("scoring exploded"),
                "adapter detail should survive to the wire; got: {message}"
            );
        }
        other => panic!("expected Error{{Internal,..}}, got {other:?}"),
    }

    d.stop().await;
}

/// A frame-level decode error is not recoverable — resyncing a corrupt
/// stream is guesswork — so the daemon answers once and closes. Same
/// posture as the generation surface (ADR 0021).
#[tokio::test]
async fn malformed_json_yields_invalid_request_then_closes() {
    let d = boot("badjson", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    conn.send_raw(b"{this is not valid json").await;
    match conn.recv().await {
        RerankResponse::Error { code, .. } => {
            assert_eq!(code, RerankErrorCode::InvalidRequest);
        }
        other => panic!("expected Error{{InvalidRequest,..}}, got {other:?}"),
    }
    assert!(
        conn.try_recv().await.is_none(),
        "connection must close after a frame-level decode error"
    );

    d.stop().await;
}

/// The connection is long-lived by design: send, read one terminal frame,
/// send the next. Pins that the loop doesn't leak per-request state (ids
/// come back in order, each with its own result set).
#[tokio::test]
async fn sequential_requests_on_one_connection_each_terminate_independently() {
    let d = boot("sequential", vec![mock_backend()], AcceptContext::default()).await;
    let mut conn = Conn::connect(&d.addr).await;

    for i in 0..3 {
        let mut req = graded_request(&format!("rr-seq-{i}"));
        req.top_n = Some(i + 1);
        match conn.round_trip(&req).await {
            RerankResponse::Rerank { id, results, .. } => {
                assert_eq!(id, format!("rr-seq-{i}"));
                assert_eq!(results.len(), (i + 1) as usize);
            }
            other => panic!("expected Rerank, got {other:?}"),
        }
    }

    // A clean client close ends the loop without an error.
    d.stop().await;
}
