//! v2 wire format — typed content blocks, attachments, tools.
//!
//! Defined by ADR 0015. Lives on a *separate* socket from v1 (per ADR
//! 0008 — breaking changes go to v2 on a new endpoint, never in-band
//! on v1). The framing layer (`MAX_FRAME_BYTES`, `read_frame`,
//! `write_frame`) is shared with v1; only the request / response
//! envelope shape differs.
//!
//! v2 is **design-locked, not yet wire-shipped**: this module is the
//! type surface middleware authors write against today; the daemon
//! gains a v2 socket in Phase 1B (returns `not_implemented` until
//! Phase 2A wires `Backend::generate_v2`).

mod attachment;
mod request;
mod response;
mod tool;

pub use attachment::Attachment;
pub use request::{ContentBlock, MessageV2, RequestV2, ResolvedV2, RoleV2};
pub use response::{ErrorCodeV2, ResponseBlock, ResponseV2, StopReasonV2, UsageV2};
pub use tool::{Tool, ToolCallId, ToolUseInput};
