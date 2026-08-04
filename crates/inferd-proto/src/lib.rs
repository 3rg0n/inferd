//! Wire format for inferd.
//!
//! See ADR 0015 for the v2 generation specification — typed content
//! blocks, attachments, tools. As of v0.4 (ADR 0021) v2 is the single
//! generation surface; the original text-only v1 wire was folded into
//! v2 and its types removed. v2 types live under [`mod@v2`].
//!
//! See ADR 0017 for the embeddings specification — single-frame
//! request/response over a third dedicated socket. Embed types live
//! under [`mod@embed`].
//!
//! See ADR 0027 for the reranking specification — a cross-encoder
//! scoring surface on a fourth socket, single-frame request/response
//! like embed but returning scored indices rather than vectors. Rerank
//! types live under [`mod@rerank`].
//!
//! See ADR 0021 for the v0.4 framing redesign: length-prefixed,
//! type-tagged frames (JSON / BLOB) replace newline-delimited JSON on
//! the generation surface. The embed surface still rides NDJSON via
//! [`read_frame`]/[`write_frame`].

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod embed;
mod error;
mod frame;
pub mod rerank;
pub mod v2;

pub use error::{ErrorCode, ProtoError};
pub use frame::{
    FrameType, MAX_FRAME_BYTES, RawFrame, decode_json_payload, read_frame, read_lp_frame,
    write_frame, write_lp_blob, write_lp_json,
};
