//! Wire format for inferd.
//!
//! See `docs/protocol-v1.md` for the authoritative specification and ADR
//! 0008 for the design rationale. v1 is immutable once shipped; breaking
//! changes go to v2 on a separate socket path.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

mod error;
mod frame;
mod request;
mod response;

pub use error::{ErrorCode, ProtoError};
pub use frame::{read_frame, write_frame, MAX_FRAME_BYTES};
pub use request::{ImageTokenBudget, Message, Request, Resolved, Role, VALID_IMAGE_TOKEN_BUDGETS};
pub use response::{Response, StopReason, Usage};
