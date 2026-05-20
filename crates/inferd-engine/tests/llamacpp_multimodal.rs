//! Tier 3 integration tests for `LlamaCpp`'s v2 multimodal path.
//!
//! Per `docs/test-strategy.md` §"Tier 3", these run end-to-end
//! against a real `libllama` build, an on-disk Gemma 4 GGUF, and an
//! on-disk multimodal projector (mmproj) blob. They are gated
//! behind the `llamacpp-integration` cargo feature and skip
//! themselves with an explanatory message if the relevant env vars
//! are not set.
//!
//! To run locally:
//! ```text
//! cargo test -p inferd-engine \
//!   --features llamacpp-integration \
//!   --test llamacpp_multimodal \
//!   -- --nocapture
//! ```
//! Required env vars:
//!   - INFERD_TEST_MODEL_PATH      — text GGUF
//!   - INFERD_TEST_MMPROJ_PATH     — multimodal projector blob
//!   - INFERD_TEST_MULTIMODAL_IMAGE (optional) — path to a JPEG/PNG
//!     the test will decode + send through the wire shape. When
//!     unset, the image-input test skips (text-only multimodal is
//!     still useful — it confirms the v2 dispatch path works
//!     against a model that *can* take multimodal but the request
//!     happens to be text-only).
//!
//! These tests do NOT make any assertion about model output text —
//! the model's actual answer is fragile across quants and seeds.
//! What they pin is wire round-trip behaviour: the v2 dispatch
//! reaches generate_v2, the mtmd path tokenizes + encodes
//! attachments, the sampler emits Text frames, and the stream
//! terminates with a Done.

#![cfg(feature = "llamacpp-integration")]

use base64::Engine as _;
use inferd_engine::llamacpp::{LlamaCpp, LlamaCppConfig};
use inferd_engine::{Backend, TokenEventV2};
use inferd_proto::v2::{
    Attachment, ContentBlock, MessageV2, RequestV2, ResolvedV2, RoleV2, StopReasonV2,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio_stream::StreamExt;

fn model_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_MODEL_PATH").map(PathBuf::from)
}

fn mmproj_path() -> Option<PathBuf> {
    std::env::var_os("INFERD_TEST_MMPROJ_PATH").map(PathBuf::from)
}

fn skipping(reason: &str) {
    eprintln!("[skip] {reason}; skipping tier-3 llamacpp v2 test. See docs/test-strategy.md.");
}

async fn collect_to_done(backend: &LlamaCpp, req: RequestV2) -> Vec<TokenEventV2> {
    let resolved: ResolvedV2 = req.resolve().expect("resolve must succeed");
    let stream = backend.generate_v2(resolved).await.expect("generate_v2");
    tokio::time::timeout(Duration::from_secs(120), stream.collect::<Vec<_>>())
        .await
        .expect("v2 generation timed out")
}

fn text_request(text: &str) -> RequestV2 {
    RequestV2 {
        id: "t-v2".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }],
        max_tokens: Some(16),
        ..Default::default()
    }
}

#[tokio::test]
async fn v2_text_only_streams_to_done() {
    let (Some(model), Some(mmproj)) = (model_path(), mmproj_path()) else {
        skipping("INFERD_TEST_MODEL_PATH or INFERD_TEST_MMPROJ_PATH not set");
        return;
    };

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: model,
        mmproj_path: Some(mmproj),
        n_ctx: 2048,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    assert!(backend.ready(), "backend must report ready after init");
    let caps = backend.capabilities();
    assert!(
        caps.v2,
        "backend with mmproj must advertise v2 = true; got {caps:?}"
    );

    let events = collect_to_done(&backend, text_request("Say hi briefly.")).await;
    assert!(!events.is_empty(), "expected at least a Done event");

    match events.last().unwrap() {
        TokenEventV2::Done { stop_reason, usage } => {
            assert!(matches!(
                *stop_reason,
                StopReasonV2::EndTurn | StopReasonV2::MaxTokens
            ));
            assert!(usage.input_tokens > 0, "expected input_tokens > 0");
            assert!(usage.output_tokens > 0, "expected output_tokens > 0");
        }
        other => panic!("expected terminal Done, got {other:?}"),
    }

    let n_text = events
        .iter()
        .filter(|e| matches!(e, TokenEventV2::Text(_)))
        .count();
    assert!(n_text > 0, "expected at least one Text event");
}

#[tokio::test]
async fn v2_image_attachment_round_trips() {
    let (Some(model), Some(mmproj)) = (model_path(), mmproj_path()) else {
        skipping("INFERD_TEST_MODEL_PATH or INFERD_TEST_MMPROJ_PATH not set");
        return;
    };
    let Some(image_path) = std::env::var_os("INFERD_TEST_MULTIMODAL_IMAGE").map(PathBuf::from)
    else {
        skipping("INFERD_TEST_MULTIMODAL_IMAGE not set");
        return;
    };

    let backend = LlamaCpp::new(LlamaCppConfig {
        model_path: model,
        mmproj_path: Some(mmproj),
        n_ctx: 4096,
        ..Default::default()
    })
    .expect("construct LlamaCpp");

    let caps = backend.capabilities();
    if !caps.vision {
        skipping("loaded mmproj does not advertise vision support");
        return;
    }

    // Decode the image to raw RGB. The test picks a small fixed
    // resolution (256x256) by reading + resizing via the `image`
    // crate, which the test has as a dev-dependency. ADR 0016 puts
    // image decoding on the consumer side; this test is acting as a
    // consumer.
    let img = image::open(&image_path).expect("open test image");
    let resized = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
    let rgb = resized.to_rgb8().into_raw();
    assert_eq!(rgb.len(), 256 * 256 * 3);

    let req = RequestV2 {
        id: "t-v2-img".into(),
        messages: vec![MessageV2 {
            role: RoleV2::User,
            content: vec![
                ContentBlock::Text {
                    text: "What's in this image? Be brief.".into(),
                },
                ContentBlock::Image {
                    attachment_id: "img".into(),
                },
            ],
        }],
        attachments: vec![Attachment::Image {
            id: "img".into(),
            width: 256,
            height: 256,
            bytes: base64::engine::general_purpose::STANDARD.encode(&rgb),
        }],
        max_tokens: Some(32),
        ..Default::default()
    };

    let events = collect_to_done(&backend, req).await;
    assert!(!events.is_empty(), "expected at least a Done event");

    let last = events.last().unwrap();
    match last {
        TokenEventV2::Done { usage, .. } => {
            // Multimodal prompts have substantially more input
            // tokens than a text-only one — pin a non-zero floor.
            assert!(
                usage.input_tokens > 50,
                "expected input_tokens > 50 for an image prompt, got {}",
                usage.input_tokens
            );
            assert!(usage.output_tokens > 0, "expected output_tokens > 0");
        }
        other => panic!("expected terminal Done, got {other:?}"),
    }
}
