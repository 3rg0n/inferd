//! Exit-criterion: end-to-end length-prefixed v2 over the daemon's UDS
//! transport with the real `LlamaCpp` backend (v0.4 / ADR 0021).
//!
//! Mirrors `tests/echo.rs` (mock backend) but uses `LlamaCpp::new`
//! against an on-disk GGUF model. Gated behind the
//! `llamacpp-integration` cargo feature; skips with an explanatory
//! message when `INFERD_TEST_MODEL_PATH` is unset.
//!
//! To run locally:
//!   set INFERD_TEST_MODEL_PATH=C:/path/to/gemma-4-e2b.Q4_K_M.gguf
//!   cargo test -p inferd-daemon \
//!     --features llamacpp-integration \
//!     --test echo_llamacpp -- --nocapture

#![cfg(feature = "llamacpp-integration")]

mod common;

#[cfg(unix)]
use common::{read_lp_frame, text_request, write_request};
#[cfg(unix)]
use inferd_daemon::endpoint::bind_uds;
#[cfg(unix)]
use inferd_daemon::lifecycle::wait_for_ready;
#[cfg(unix)]
use inferd_daemon::lifecycle_v2::{AcceptContext, serve_uds_v2};
#[cfg(unix)]
use inferd_daemon::router::Router;
#[cfg(unix)]
use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
#[cfg(unix)]
use inferd_proto::v2::{ResponseV2, StopReasonV2};
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(unix)]
fn model_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_MODEL_PATH").map(PathBuf::from)
}

#[cfg(unix)]
fn skipping_msg() {
    eprintln!(
        "[skip] INFERD_TEST_MODEL_PATH not set; skipping real-model daemon \
         integration test. See docs/test-strategy.md."
    );
}

#[cfg(unix)]
#[tokio::test]
async fn end_to_end_real_inference_over_uds() {
    let Some(path) = model_path() else {
        skipping_msg();
        return;
    };

    // Boot the daemon with a real LlamaCpp adapter.
    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: path,
        n_ctx: 2048,
        ..Default::default()
    })
    .expect("LlamaCpp construct");
    let backend: Arc<dyn inferd_engine::Backend> = Arc::new(backend);
    let router = Arc::new(Router::new(vec![backend]));

    wait_for_ready(&router, Duration::from_secs(60))
        .await
        .expect("backend ready");

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let idx = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let socket_path = std::env::temp_dir().join(format!(
        "inferd-test-echo-llama-{}-{}.sock",
        std::process::id(),
        idx
    ));
    let _ = std::fs::remove_file(&socket_path);

    let listener = bind_uds(&socket_path, None).await.expect("bind uds");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_uds_v2(listener, router, AcceptContext::default(), shutdown_rx).await;
    });

    // One short request. Sampling fields stay at the backend defaults.
    let mut req = text_request("m2c-1", "Say hi briefly.");
    req.temperature = Some(0.7);
    req.top_p = Some(0.95);
    req.top_k = Some(40);
    req.max_tokens = Some(16);

    let mut stream = UnixStream::connect(&socket_path).await.expect("connect");
    write_request(&mut stream, &req).await;

    let (read_half, _w) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut frames = Vec::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(120), read_lp_frame(&mut reader))
            .await
            .expect("response timeout");
        let Some((_type, payload)) = next else {
            break;
        };
        let resp: ResponseV2 = serde_json::from_slice(&payload).expect("decode");
        let terminal = resp.is_terminal();
        frames.push(resp);
        if terminal {
            break;
        }
    }

    assert!(!frames.is_empty(), "expected at least one response frame");
    let last = frames.last().unwrap();
    match last {
        ResponseV2::Done {
            id,
            stop_reason,
            backend,
            usage,
        } => {
            assert_eq!(id, "m2c-1");
            assert_eq!(backend, "llamacpp");
            assert!(matches!(
                *stop_reason,
                StopReasonV2::EndTurn | StopReasonV2::MaxTokens
            ));
            assert!(
                usage.output_tokens > 0,
                "expected output_tokens > 0, got {}",
                usage.output_tokens
            );
        }
        other => panic!("expected terminal Done frame, got {other:?}"),
    }

    // Frame frames carry incremental text.
    let text_count = frames
        .iter()
        .filter(|f| matches!(f, ResponseV2::Frame { .. }))
        .count();
    assert!(text_count > 0, "expected at least one Frame{{Text}} frame");

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}
