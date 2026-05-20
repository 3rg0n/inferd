//! Wire format for inferd.
//!
//! See `docs/protocol-v1.md` for the authoritative specification of v1
//! and ADR 0008 for the design rationale. v1 is immutable once
//! shipped; breaking changes go to v2 on a separate socket path.
//!
//! See ADR 0015 for the v2 specification — typed content blocks,
//! attachments, tools. v2 types live under [`mod@v2`].

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod error;
mod frame;
mod request;
mod response;
pub mod v2;

pub use error::{ErrorCode, ProtoError};
pub use frame::{MAX_FRAME_BYTES, read_frame, write_frame};
pub use request::{ImageTokenBudget, Message, Request, Resolved, Role, VALID_IMAGE_TOKEN_BUDGETS};
pub use response::{Response, StopReason, Usage};
