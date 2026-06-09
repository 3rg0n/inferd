//! Wire format for inferd.
//!
//! See `docs/protocol-v1.md` for the authoritative specification of v1
//! and ADR 0008 for the design rationale. v1 is immutable once
//! shipped; breaking changes go to v2 on a separate socket path.
//!
//! See ADR 0015 for the v2 specification — typed content blocks,
//! attachments, tools. v2 types live under [`mod@v2`].
//!
//! See ADR 0017 for the embeddings specification — single-frame
//! request/response over a third dedicated socket. Embed types live
//! under [`mod@embed`].

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod embed;
mod error;
mod frame;
mod request;
mod response;
pub mod v2;

pub use error::{ErrorCode, ProtoError};
pub use frame::{
    FrameType, MAX_FRAME_BYTES, RawFrame, decode_json_payload, read_frame, read_lp_frame,
    write_frame, write_lp_blob, write_lp_json,
};
pub use request::{ImageTokenBudget, Message, Request, Resolved, Role, VALID_IMAGE_TOKEN_BUDGETS};
pub use response::{Response, StopReason, Usage};
