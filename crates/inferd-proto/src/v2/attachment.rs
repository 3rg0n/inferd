//! v2 attachment table — binary payloads referenced by content blocks.
//!
//! Per ADR 0015 §"v2 Attachment", as amended by ADR 0016 (consumer
//! decodes media before sending). Attachments are sent once at the
//! request envelope's top level and referenced by `id` from any
//! number of `image` / `audio` / `video` content blocks across the
//! request's `messages[]`. This indirection matches the Anthropic
//! shape and lets a multi-image conversation avoid duplicating bytes.
//!
//! ## Decode posture (ADR 0013 + ADR 0016)
//!
//! The wire carries **already-decoded** binary payloads — raw RGB
//! interleaved bytes for images, float32 PCM samples for audio.
//! The daemon does *not* link image/audio codec libraries; consumer
//! middleware decodes before sending. This matches ADR 0013's
//! gateway framing ("middleware owns the bytes") and matches what
//! libmtmd's C API expects (`mtmd_bitmap_init` takes `nx * ny * 3`
//! interleaved RGB; `mtmd_bitmap_init_from_audio` takes a float32
//! PCM slice).
//!
//! Each attachment kind carries the metadata it needs:
//!   - `Image`: `width`, `height` (the daemon recomputes nothing).
//!   - `Audio`: `sample_rate` (Hz; the daemon doesn't resample).
//!   - `Video`: reserved; the actual shape is TBD when a video-
//!     capable adapter lands.

use serde::{Deserialize, Serialize};

/// One binary attachment in the request's top-level `attachments[]` table.
///
/// Tagged-enum shape: each variant carries exactly the metadata libmtmd
/// (and other engines' multimodal interfaces) need for that modality.
/// Unknown variants deserialise as [`Attachment::Unknown`] so v2.0
/// clients don't reject newer payloads at parse time; resolve()
/// rejects them only when they reach validation.
///
/// `id` must be unique within a single request; content blocks
/// reference attachments by exactly this string.
///
/// `bytes` is standard-base64-encoded (RFC 4648, with `+/` and `=`
/// padding). After ~1.33× inflation the raw payload must still leave
/// room within the 64 MiB per-frame cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Attachment {
    /// Decoded RGB image. `bytes` is `width * height * 3` interleaved
    /// RGB octets (no alpha channel; consumer drops alpha or
    /// composites against a known background before sending).
    Image {
        /// Caller-chosen identifier; unique within the request.
        id: String,
        /// Image width in pixels.
        width: u32,
        /// Image height in pixels.
        height: u32,
        /// Base64 of `width * height * 3` interleaved RGB bytes.
        bytes: String,
    },
    /// Decoded audio PCM. `bytes` is base64 of `n_samples *
    /// sizeof(f32)` little-endian float32 samples at the named
    /// sample rate.
    Audio {
        /// Caller-chosen identifier; unique within the request.
        id: String,
        /// Sample rate in Hz (e.g. 16000 for Whisper-class encoders;
        /// Gemma 4 audio uses its own rate which the daemon learns at
        /// adapter init time and reports via
        /// `BackendCapabilities`).
        sample_rate: u32,
        /// Base64 of float32 PCM samples (little-endian).
        bytes: String,
    },
    /// Reserved. Engine support is a separate concern; v2.0 daemons
    /// reject video attachments with `attachment_unsupported` until
    /// a video-capable adapter ships. Wire shape is intentionally
    /// kept stub-thin; future revisions add fields without breaking
    /// v2.0 clients (forward-compat: serde will accept extra fields
    /// silently).
    Video {
        /// Caller-chosen identifier; unique within the request.
        id: String,
        /// Base64 of decoded video frames; precise format TBD.
        bytes: String,
    },
    /// Forward-compat escape hatch — any `kind` value the local build
    /// doesn't recognise lands here so older clients/daemons don't
    /// reject newer payloads at parse time. `resolve()` rejects them
    /// only when they reach validation.
    #[serde(other)]
    Unknown,
}

impl Attachment {
    /// The attachment's id (independent of variant).
    ///
    /// Returns an empty string for `Unknown` since unknown variants
    /// don't carry an id field reliably.
    pub fn id(&self) -> &str {
        match self {
            Attachment::Image { id, .. }
            | Attachment::Audio { id, .. }
            | Attachment::Video { id, .. } => id,
            Attachment::Unknown => "",
        }
    }

    /// `true` if this attachment is an image.
    pub fn is_image(&self) -> bool {
        matches!(self, Attachment::Image { .. })
    }

    /// `true` if this attachment is audio.
    pub fn is_audio(&self) -> bool {
        matches!(self, Attachment::Audio { .. })
    }

    /// `true` if this attachment is video.
    pub fn is_video(&self) -> bool {
        matches!(self, Attachment::Video { .. })
    }
}
