//! v2 wire format — typed content blocks, attachments, tools.
//!
//! Originally ADR 0015. As of v0.4 (ADR 0021) v2 is the **single**
//! generation surface — v1 was folded in and its socket removed — and
//! it rides the length-prefixed, type-tagged framing
//! (`read_lp_frame` / `write_lp_json` / `write_lp_blob`), not the old
//! newline-delimited codec. Attachment bytes travel out-of-band in BLOB
//! frames keyed by id; the JSON request carries only attachment
//! metadata. A `wire_version` on the request + capabilities frame makes
//! schema mismatch fail loudly.

mod attachment;
mod request;
mod response;
mod tool;

/// The wire-format version this build speaks (ADR 0021). Sent on every
/// `RequestV2` and advertised in the capabilities frame; a daemon
/// rejects a request whose `wire_version` it doesn't support with
/// `ErrorCodeV2::WireVersionUnsupported`. Starts at `1` for the v0.4
/// length-prefixed framing (the pre-v0.4 newline framing is version 0,
/// which v0.4 does not accept).
pub const WIRE_VERSION: u32 = 1;

pub use attachment::{Attachment, BlobDescriptor, BlobDescriptorTag};
pub use request::{ContentBlock, MessageV2, RequestV2, ResolvedV2, ResponseFormat, RoleV2};
pub use response::{ErrorCodeV2, ResponseBlock, ResponseV2, StopReasonV2, UsageV2};
pub use tool::{Tool, ToolCallId, ToolUseInput};
