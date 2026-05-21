//! Rust client for the inferd local-inference daemon.
//!
//! Wire protocol is NDJSON over Unix socket / Windows named pipe /
//! loopback TCP. Spec is frozen as protocol v1; see the inferd
//! repository's `docs/protocol-v1.md`.
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
//! ## Quickstart (v1)
//!
//! ```no_run
//! use inferd_client::{Client, Request, Message, Role, Response};
//! use tokio_stream::StreamExt;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = inferd_client::dial_and_wait_ready(
//!     std::time::Duration::from_secs(30),
//!     || Client::dial_tcp("127.0.0.1:47321"),
//! )
//! .await?;
//!
//! let mut stream = client.generate(Request {
//!     id: "demo-1".into(),
//!     messages: vec![Message {
//!         role: Role::User,
//!         content: "hello".into(),
//!     }],
//!     ..Default::default()
//! })
//! .await?;
//!
//! while let Some(frame) = stream.next().await {
//!     match frame? {
//!         Response::Token { content, .. } => print!("{content}"),
//!         Response::Done { stop_reason, backend, .. } => {
//!             println!("\n[done; backend={backend}, stop={stop_reason:?}]");
//!         }
//!         Response::Error { code, message, .. } => {
//!             eprintln!("[error {code:?}: {message}]");
//!         }
//!         Response::Status { .. } => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Quickstart (v2 — typed content blocks, attachments, tools)
//!
//! v2 lives on a *separate* socket from v1 per ADR 0015. Use
//! [`ClientV2`] with `dial_v2_*` instead of `dial_tcp`/`dial_uds` and
//! the v2 wire types (`RequestV2`, `ContentBlock`, …).
//!
//! ```no_run
//! use inferd_client::{ClientV2, RequestV2, MessageV2, RoleV2, ContentBlock, ResponseV2, ResponseBlock};
//! use tokio_stream::StreamExt;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = inferd_client::dial_and_wait_ready(
//!     std::time::Duration::from_secs(30),
//!     || ClientV2::dial_tcp("127.0.0.1:47322"),
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

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod admin;
mod client;
mod v2_client;
mod wait;

pub use admin::{AdminClient, AdminEvent};
pub use client::{Client, ClientError, FrameStream};
pub use v2_client::{ClientV2, FrameStreamV2, default_v2_addr};
pub use wait::{WaitError, default_admin_addr, dial_and_wait_ready, is_transient_dial_error};

/// Re-exports from `inferd-proto` so consumers don't need a separate
/// `inferd-proto` dep for the wire types. The proto crate IS the
/// version-pin contract for protocol compatibility — `inferd-client
/// 0.2` always uses `inferd-proto 0.2`.
pub use inferd_proto::{
    ErrorCode, ImageTokenBudget, MAX_FRAME_BYTES, Message, ProtoError, Request, Resolved, Response,
    Role, StopReason, Usage, VALID_IMAGE_TOKEN_BUDGETS,
};

/// Re-exports of the v2 wire types per ADR 0015. v2 is shipped as
/// part of `inferd-client 0.2` so consumers building against v2 can
/// reach the proto types without a separate `inferd-proto` dep.
pub use inferd_proto::v2::{
    Attachment, ContentBlock, ErrorCodeV2, MessageV2, RequestV2, ResolvedV2, ResponseBlock,
    ResponseV2, RoleV2, StopReasonV2, Tool, ToolCallId, ToolUseInput, UsageV2,
};
