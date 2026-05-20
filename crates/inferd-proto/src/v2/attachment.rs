//! v2 attachment table — binary blobs referenced by content blocks.
//!
//! Per ADR 0015 §"v2 Attachment". Attachments are sent once at the
//! request envelope's top level and referenced by `id` from any
//! number of `image` / `audio` / `video` content blocks across the
//! request's `messages[]`. This indirection matches the Anthropic
//! shape and lets a multi-image conversation avoid duplicating
//! bytes.

use serde::{Deserialize, Serialize};

/// Attachment-kind discriminant. Determines which engine side-channel
/// the daemon will route the bytes through (e.g. mtmd's `mtmd_bitmap_init`
/// vs `mtmd_bitmap_init_from_audio`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    /// Image bytes. MIME determines decode (image/jpeg, image/png, etc.).
    Image,
    /// Audio bytes. The active backend's `BackendCapabilities` reports the
    /// expected sample rate; middleware is responsible for resampling.
    Audio,
    /// Video bytes. Engine support is gated by adapter capabilities;
    /// reserved on the wire so v2.0 clients don't have to migrate when
    /// video-capable engines arrive.
    Video,
}

/// One binary attachment in the request's top-level `attachments[]` table.
///
/// `bytes` is base64-encoded raw bytes. The base64 alphabet is the
/// standard one (`+/`); padded. After base64 inflation (~1.33×) the
/// raw payload must still leave room within the 64 MiB per-frame cap.
///
/// `id` must be unique within a single request; content blocks
/// reference attachments by exactly this string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Caller-chosen identifier referenced from `ContentBlock::Image { attachment_id }`,
    /// `ContentBlock::Audio { attachment_id }`, etc. Must be unique within
    /// the enclosing request.
    pub id: String,
    /// Discriminant for which engine side-channel routes this blob.
    pub kind: AttachmentKind,
    /// Best-effort MIME type. The daemon validates against the active
    /// backend's `BackendCapabilities`; unsupported MIMEs trigger
    /// `attachment_unsupported`.
    pub mime: String,
    /// Standard-base64-encoded raw bytes.
    pub bytes: String,
}
