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

/// Maximum attachments a single request may declare.
///
/// The 64 MiB frame cap ([`crate::MAX_FRAME_BYTES`], THREAT_MODEL F-5)
/// bounds one *frame*, not one *request*: each declared attachment
/// entitles the sender to send one further BLOB frame, so an unbounded
/// attachment table turns one in-cap request frame into an unbounded
/// amount of reads (THREAT_MODEL F-1). This bounds the multiplier.
///
/// Sized to admit what a legitimate producer sends — `inferd-http`'s
/// `MAX_IMAGES_PER_REQUEST` is 8 — with headroom for multi-modal
/// conversations that reference several images plus audio.
pub const MAX_ATTACHMENTS_PER_REQUEST: usize = 32;

/// Maximum total attachment bytes a single request may carry, summed
/// across every BLOB frame.
///
/// The companion to [`MAX_ATTACHMENTS_PER_REQUEST`]: the count cap alone
/// still permits `count × 64 MiB`. Readers enforce this against the
/// *declared* `BlobDescriptor::len` before reading a payload, so an
/// over-budget request costs no heap.
///
/// Matches `inferd-http`'s `MAX_TOTAL_DECODED_IMAGE_BYTES` (128 MiB) so
/// the bridge cannot produce a request the daemon refuses.
pub const MAX_ATTACHMENT_BYTES_PER_REQUEST: u64 = 128 * 1024 * 1024;

// These caps must never reject what a legitimate producer sends. The
// tightest such producer is the `inferd-http` bridge, whose own limits are
// `MAX_IMAGES_PER_REQUEST` = 8 and `MAX_TOTAL_DECODED_IMAGE_BYTES` = 128
// MiB; lowering either cap below those would make the daemon refuse
// requests the bridge happily builds. Compile-time so the break is a build
// failure, not a runtime surprise on a vision request.
const _: () = assert!(
    MAX_ATTACHMENTS_PER_REQUEST >= 8,
    "cap is below inferd-http's MAX_IMAGES_PER_REQUEST (8); the daemon would refuse requests the bridge builds"
);
const _: () = assert!(
    MAX_ATTACHMENT_BYTES_PER_REQUEST >= 128 * 1024 * 1024,
    "cap is below inferd-http's MAX_TOTAL_DECODED_IMAGE_BYTES (128 MiB); the daemon would refuse requests the bridge builds"
);

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
/// ## Bytes ride out-of-band (ADR 0021)
///
/// As of the v0.4 length-prefixed framing, the raw payload does **not**
/// travel inside this JSON object. The JSON attachment carries only
/// metadata (`kind` / `id` / `width` / `height` / `sample_rate`); the
/// decoded bytes are sent in a separate length-prefixed BLOB frame,
/// each preceded by a [`BlobDescriptor`] naming the `id`. The `bytes`
/// field below is therefore `#[serde(skip)]` — `Vec<u8>` of *raw*
/// decoded octets (no base64), populated in-memory by the producer
/// before sending the BLOB and by the daemon after reading it. This
/// kills the +33% base64 tax and lets the daemon hand raw RGB straight
/// to mtmd. Decode posture is unchanged (ADR 0016): the consumer still
/// decodes media to raw bytes; only the transport moved.
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
        /// Raw `width * height * 3` interleaved RGB bytes. Carried in a
        /// BLOB frame, not this JSON object.
        #[serde(skip)]
        bytes: Vec<u8>,
    },
    /// Decoded audio PCM: `bytes` is `n_samples * sizeof(f32)`
    /// little-endian float32 samples at the named sample rate.
    Audio {
        /// Caller-chosen identifier; unique within the request.
        id: String,
        /// Sample rate in Hz (e.g. 16000 for Whisper-class encoders;
        /// Gemma 4 audio uses its own rate which the daemon learns at
        /// adapter init time and reports via
        /// `BackendCapabilities`).
        sample_rate: u32,
        /// Raw little-endian float32 PCM bytes. Carried in a BLOB frame.
        #[serde(skip)]
        bytes: Vec<u8>,
    },
    /// Reserved. Engine support is a separate concern; daemons reject
    /// video attachments with `attachment_unsupported` until a
    /// video-capable adapter ships. Wire shape is intentionally kept
    /// stub-thin; future revisions add fields without breaking older
    /// clients (forward-compat: serde accepts extra fields silently).
    Video {
        /// Caller-chosen identifier; unique within the request.
        id: String,
        /// Raw decoded video bytes; precise format TBD. Carried in a
        /// BLOB frame.
        #[serde(skip)]
        bytes: Vec<u8>,
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

    /// The raw (decoded, un-base64'd) attachment bytes. Empty for
    /// `Unknown`, and empty on a freshly-deserialised attachment until
    /// the matching BLOB frame has been applied via [`set_bytes`].
    ///
    /// [`set_bytes`]: Attachment::set_bytes
    pub fn bytes(&self) -> &[u8] {
        match self {
            Attachment::Image { bytes, .. }
            | Attachment::Audio { bytes, .. }
            | Attachment::Video { bytes, .. } => bytes,
            Attachment::Unknown => &[],
        }
    }

    /// Install the raw bytes read from the attachment's BLOB frame. The
    /// daemon calls this after reading the BLOB whose descriptor named
    /// this attachment's id. No-op on `Unknown`.
    pub fn set_bytes(&mut self, raw: Vec<u8>) {
        match self {
            Attachment::Image { bytes, .. }
            | Attachment::Audio { bytes, .. }
            | Attachment::Video { bytes, .. } => *bytes = raw,
            Attachment::Unknown => {}
        }
    }
}

/// Descriptor JSON frame that precedes each attachment BLOB frame
/// (ADR 0021). The producer sends, per attachment: this descriptor
/// (a JSON control frame) naming the `attachment_id` and the BLOB's
/// byte length, then the BLOB frame itself. The reader uses `id` to
/// correlate the bytes to the attachment in the already-received
/// `RequestV2`, and `len` to sanity-check the BLOB it reads next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobDescriptor {
    /// Discriminates this control frame on the wire.
    #[serde(rename = "type")]
    pub frame_kind: BlobDescriptorTag,
    /// The `Attachment::id` the following BLOB frame's bytes belong to.
    pub attachment_id: String,
    /// Expected byte length of the following BLOB frame.
    pub len: u64,
}

/// The `type` tag value identifying a [`BlobDescriptor`] JSON frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobDescriptorTag {
    /// Marks the frame as an attachment-blob descriptor.
    AttachmentBlob,
}

impl BlobDescriptor {
    /// Build a descriptor for `attachment_id` covering `len` BLOB bytes.
    pub fn new(attachment_id: impl Into<String>, len: u64) -> Self {
        Self {
            frame_kind: BlobDescriptorTag::AttachmentBlob,
            attachment_id: attachment_id.into(),
            len,
        }
    }
}
