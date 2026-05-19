//! M2c exit-criterion: end-to-end NDJSON over the daemon's TCP
//! transport with the real `LlamaCpp` backend.
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

use inferd_daemon::endpoint::bind_tcp;
use inferd_daemon::lifecycle::{AcceptContext, serve_tcp, wait_for_ready};
use inferd_daemon::router::Router;
use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_proto::{Message, Request, Response, Role, StopReason, write_frame};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

fn model_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_MODEL_PATH").map(PathBuf::from)
}

fn skipping_msg() {
    eprintln!(
        "[skip] INFERD_TEST_MODEL_PATH not set; skipping M2c daemon \
         integration test. See docs/test-strategy.md."
    );
}

#[tokio::test]
async fn end_to_end_real_inference_over_tcp() {
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

    let listener = bind_tcp("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap().to_string();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = serve_tcp(listener, router, AcceptContext::default(), shutdown_rx).await;
    });

    // One short request.
    let req = Request {
        id: "m2c-1".into(),
        messages: vec![Message {
            role: Role::User,
            content: "Say hi briefly.".into(),
        }],
        temperature: Some(0.7),
        top_p: Some(0.95),
        top_k: Some(40),
        max_tokens: Some(16),
        stream: Some(true),
        image_token_budget: None,
        grammar: String::new(),
    };

    // Connect and drive.
    let mut stream = TcpStream::connect(&addr).await.expect("connect");
    let mut buf = Vec::new();
    write_frame(&mut buf, &req).expect("write frame");
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    let mut reader = BufReader::new(stream);
    let mut frames = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = tokio::time::timeout(
            Duration::from_secs(120),
            reader.read_until(b'\n', &mut line),
        )
        .await
        .expect("response timeout")
        .expect("read");
        if n == 0 {
            break;
        }
        let resp: Response = serde_json::from_slice(&line).expect("decode");
        let terminal = resp.is_terminal();
        frames.push(resp);
        if terminal {
            break;
        }
    }

    assert!(!frames.is_empty(), "expected at least one response frame");
    let last = frames.last().unwrap();
    match last {
        Response::Done {
            id,
            stop_reason,
            backend,
            usage,
            ..
        } => {
            assert_eq!(id, "m2c-1");
            assert_eq!(backend, "llamacpp");
            assert!(matches!(*stop_reason, StopReason::End | StopReason::Length));
            assert!(
                usage.completion_tokens > 0,
                "expected completion_tokens > 0, got {}",
                usage.completion_tokens
            );
        }
        other => panic!("expected terminal Done frame, got {other:?}"),
    }

    // Token frames carry incremental text.
    let token_count = frames
        .iter()
        .filter(|f| matches!(f, Response::Token { .. }))
        .count();
    assert!(token_count > 0, "expected at least one Token frame");

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}
