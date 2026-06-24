//! Rust client for the inferd local-inference daemon.
//!
//! Wire protocol is length-prefixed, type-tagged frames over Unix
//! socket / Windows named pipe / loopback TCP for generation (v2,
//! ADR 0021) and NDJSON for embeddings (ADR 0017). As of v0.4 the
//! text-only v1 generation surface was folded into v2 and removed;
//! [`ClientV2`] is the single generation client.
//!
//! Two patterns for waiting on the daemon to come up; pick based on
//! whether you need progress UX:
//!
//! - **Pattern A (passive)** — [`dial_and_wait_ready`] retries
//!   connect against the inference transport with exponential
//!   backoff. Successful connect is the ready signal because the
//!   daemon's inference socket only exists when the backend is ready
//!   (THREAT_MODEL F-13 in the upstream repo). Standard
//!   Postgres/Redis/etcd client shape.
//! - **Pattern B (active)** — [`AdminClient`] subscribes to the
//!   admin socket and yields lifecycle events
//!   (`starting`/`loading_model`/`ready`/`restarting`/`draining`).
//!   Use this for installer GUIs, dashboards, or middleware that
//!   wants to display download progress during first-boot
//!   bootstrap.
//!
//! ## Quickstart (v2 — typed content blocks, attachments, tools)
//!
//! v2 is the single generation surface (ADR 0021). Use [`ClientV2`]
//! with `dial_v2_*` and the v2 wire types (`RequestV2`,
//! `ContentBlock`, …). A text-only request is a single `Text`
//! content block.
//!
//! ```ignore
//! use inferd_client::{ClientV2, RequestV2, MessageV2, RoleV2, ContentBlock, ResponseV2, ResponseBlock};
//! use tokio_stream::StreamExt;
//! use std::path::Path;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = inferd_client::dial_and_wait_ready(
//!     std::time::Duration::from_secs(30),
//!     || ClientV2::dial_uds(Path::new("/tmp/inferd/inferd.sock")),
//! )
//! .await?;
//!
//! let mut stream = client.generate(RequestV2 {
//!     id: "demo-1".into(),
//!     messages: vec![MessageV2 {
//!         role: RoleV2::User,
//!         content: vec![ContentBlock::Text { text: "hello".into() }],
//!     }],
//!     ..Default::default()
//! })
//! .await?;
//!
//! while let Some(frame) = stream.next().await {
//!     match frame? {
//!         ResponseV2::Frame { block: ResponseBlock::Text { delta }, .. } => print!("{delta}"),
//!         ResponseV2::Frame { block: ResponseBlock::Thinking { .. }, .. } => {}
//!         ResponseV2::Frame { block: ResponseBlock::ToolUse { name, .. }, .. } => {
//!             println!("\n[tool_use: {name}]");
//!         }
//!         ResponseV2::Done { stop_reason, backend, .. } => {
//!             println!("\n[done; backend={backend}, stop={stop_reason:?}]");
//!         }
//!         ResponseV2::Error { code, message, .. } => {
//!             eprintln!("[error {code:?}: {message}]");
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Quickstart (embed — single-frame request/response)
//!
//! Embed lives on a *third* socket separate from v1 and v2 per ADR
//! 0017. Use [`EmbedClient`] with `dial_embed_*` and the embed wire
//! types (`EmbedRequest`, `EmbedResponse`, `EmbedTask`, …). The call
//! is a single round-trip — no streaming, since an embedding is a
//! complete vector.
//!
//! ```ignore
//! use inferd_client::{EmbedClient, EmbedRequest, EmbedResponse, EmbedTask};
//! use std::path::Path;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = inferd_client::dial_and_wait_ready(
//!     std::time::Duration::from_secs(30),
//!     || EmbedClient::dial_uds(Path::new("/tmp/inferd/infer.embed.sock")),
//! )
//! .await?;
//!
//! let resp = client.embed(EmbedRequest {
//!     id: "demo-1".into(),
//!     input: vec!["the quick brown fox".into()],
//!     dimensions: Some(256),
//!     task: Some(EmbedTask::RetrievalDocument),
//! })
//! .await?;
//!
//! match resp {
//!     EmbedResponse::Embeddings { embeddings, dimensions, .. } => {
//!         println!("got {} vectors of dim {dimensions}", embeddings.len());
//!     }
//!     EmbedResponse::Error { code, message, .. } => {
//!         eprintln!("[embed error {code:?}: {message}]");
//!     }
//! }
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod admin;
mod client;
mod embed_client;
mod v2_client;
mod wait;

pub use admin::{AdminClient, AdminEvent};
pub use client::ClientError;
pub use embed_client::{EmbedClient, default_embed_addr};
pub use v2_client::{ClientV2, FrameStreamV2, default_v2_addr};
pub use wait::{WaitError, default_admin_addr, dial_and_wait_ready, is_transient_dial_error};

/// Re-exports from `inferd-proto` so consumers don't need a separate
/// `inferd-proto` dep for the wire types. The proto crate IS the
/// version-pin contract for protocol compatibility — `inferd-client
/// 0.4` always uses `inferd-proto 0.4`.
pub use inferd_proto::{ErrorCode, MAX_FRAME_BYTES, ProtoError};

/// Re-exports of the v2 wire types per ADR 0015 — the single
/// generation surface as of v0.4 (ADR 0021). Shipped as part of
/// `inferd-client` so consumers building against v2 can reach the
/// proto types without a separate `inferd-proto` dep.
pub use inferd_proto::v2::{
    Attachment, ContentBlock, ErrorCodeV2, MessageV2, RequestV2, ResolvedV2, ResponseBlock,
    ResponseV2, RoleV2, StopReasonV2, Tool, ToolCallId, ToolUseInput, UsageV2,
};

/// Re-exports of the embed wire types per ADR 0017. Embed lives on
/// the *third* inferd socket (separate from v1 and v2); the
/// proto types are re-exported here so consumers don't need a separate
/// `inferd-proto` dep.
pub use inferd_proto::embed::{
    EmbedErrorCode, EmbedRequest, EmbedResolved, EmbedResponse, EmbedTask, EmbedUsage,
};
