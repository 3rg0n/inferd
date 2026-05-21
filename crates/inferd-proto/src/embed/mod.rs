//! Embeddings wire format.
//!
//! Defined by ADR 0017. Lives on a *separate* socket from v1 and v2
//! (`infer.embed.sock` / `\\.\pipe\inferd-infer-embed`) — the same
//! "separate-socket-per-surface" rule ADR 0008 established for v2.
//!
//! Single-frame request, single-frame response. Long-lived connection
//! semantics match v1 / v2: send a `Request`, receive one terminal
//! `Response`, send another `Request`. No streaming, since an
//! embedding is a complete vector — there is nothing to stream.
//!
//! Framing layer (`MAX_FRAME_BYTES`, `read_frame`, `write_frame`) is
//! shared with v1 / v2; only the request / response envelope shape
//! differs.

mod request;
mod response;

pub use request::{EmbedRequest, EmbedResolved, EmbedTask};
pub use response::{EmbedErrorCode, EmbedResponse, EmbedUsage};
