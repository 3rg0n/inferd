//! Rerank wire format.
//!
//! Defined by ADR 0027. Lives on a *fourth* socket, separate from
//! generation and embed (`infer.rerank.sock` /
//! `\\.\pipe\inferd-infer-rerank`) — the same
//! "separate-socket-per-surface" rule ADR 0008 established for v2 and
//! ADR 0017 reused for embed.
//!
//! Single-frame request, single-frame response, NDJSON framing shared
//! with [`crate::embed`]. Nothing streams: a rerank result is a
//! complete ordering, and a partial ordering is not useful.
//!
//! ## Rerank is not embed
//!
//! Embed is a *bi-encoder* surface — each input is encoded
//! independently, so vectors can be precomputed and indexed. Rerank is
//! a *cross-encoder*: query and document go through one forward pass
//! **together**, so the score can depend on their interaction (which
//! is what lets it see negation and narrow intent that cosine
//! similarity over independent vectors loses). The cost of that is
//! that nothing can be precomputed, so rerank is the reordering stage
//! over a candidate set embed already narrowed:
//!
//! ```text
//! embed → top-50 candidates → rerank(query, 50 docs) → top-5 → LLM
//! ```
//!
//! That asymmetry is why the two surfaces do not share an envelope:
//! embed returns one vector per input, rerank returns scored *indices*
//! into the request's `documents`.

mod request;
mod response;

pub use request::{MAX_RERANK_DOCUMENTS, MAX_RERANK_TOTAL_BYTES, RerankRequest, RerankResolved};
pub use response::{RerankErrorCode, RerankResponse, RerankResult, RerankUsage};
