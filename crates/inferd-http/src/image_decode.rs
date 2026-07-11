//! Decode an OpenAI `image_url` content part into raw interleaved RGB
//! bytes for an inferd image attachment.
//!
//! inferd's daemon links **no image codec** (ADR 0016): an image
//! attachment travels the wire as raw `width * height * 3` interleaved
//! RGB octets, and the *consumer* is responsible for decoding the source
//! format. This bridge is that consumer for OpenAI clients, so it decodes
//! the PNG/JPEG the client sends here and hands the daemon raw RGB.
//!
//! ## Security posture
//!
//! Two attacker-controlled surfaces are guarded here:
//!
//! 1. **SSRF.** OpenAI's `image_url.url` may be a remote `http(s)://`
//!    URL. A server-side fetch of an arbitrary URL would let a client
//!    make this process issue requests to internal hosts / cloud
//!    metadata endpoints. inferd's bridge therefore accepts **only
//!    `data:` URLs** (the image inline) and rejects remote URLs with a
//!    clear error. A future opt-in could allow an operator-configured
//!    allowlist, but silently fetching is not acceptable.
//!
//! 2. **Decompression bombs.** A few-KB PNG can decode to gigabytes of
//!    RGB. We cap both the encoded payload size and the decoded pixel
//!    dimensions via `image::Limits` before decoding, so a hostile image
//!    can't exhaust memory.

use base64::Engine as _;
use image::ImageReader;
use std::io::Cursor;

/// Raw decoded image ready to become an inferd attachment.
#[derive(Debug)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 3` interleaved RGB octets (no alpha).
    pub rgb: Vec<u8>,
}

/// Failures decoding an `image_url` part → HTTP 400.
#[derive(Debug, thiserror::Error)]
pub enum ImageDecodeError {
    /// The URL was a remote `http(s)://` URL — refused (SSRF guard).
    #[error(
        "remote image URLs are not supported; inline the image as a \
         data: URL (base64). (A server-side fetch of an arbitrary URL is \
         disabled to avoid SSRF.)"
    )]
    RemoteUrlUnsupported,
    /// The URL was neither a `data:` URL nor `http(s)://`.
    #[error("unsupported image url scheme (expected a data: URL)")]
    BadScheme,
    /// A `data:` URL that wasn't `;base64,`-encoded, or was malformed.
    #[error("malformed data: URL ({0})")]
    MalformedDataUrl(&'static str),
    /// The base64 payload didn't decode.
    #[error("image base64 decode failed: {0}")]
    Base64(String),
    /// The encoded image exceeded the byte cap before decoding.
    #[error("image too large: {0} bytes exceeds the {1}-byte cap")]
    TooLargeEncoded(usize, usize),
    /// The image failed to decode, or exceeded the pixel/dimension cap.
    #[error("image decode failed: {0}")]
    Decode(String),
}

/// Max encoded (compressed) image bytes accepted. The HTTP body cap
/// (8 MiB) bounds the whole request; this bounds one image within it.
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
/// Max decoded pixels per side. 4096 (16 MP) covers any realistic
/// document scan or photo, and — critically — bounds one decoded image
/// to `4096*4096*3` = 48 MiB, which stays under the daemon's 64 MiB
/// per-frame BLOB cap. A larger image would be rejected downstream as
/// `frame_too_large`, so rejecting it here (with a clear message) beats
/// decoding 100s of MiB only to have the daemon refuse the frame. The
/// daemon owns any further downscaling toward the projector's token
/// budget (ADR 0013) — the wire still carries the full RGB.
pub const MAX_DIM: u32 = 4096;
/// Max total decoded-buffer allocation the decoder may request, as a
/// belt-and-suspenders bound alongside `MAX_DIM` (a decoder that ignored
/// the dimension limit still can't allocate past this).
const MAX_ALLOC_BYTES: u64 = 64 * 1024 * 1024;

/// Decode an `image_url.url` string into raw RGB.
///
/// Only `data:<mime>;base64,<payload>` URLs are accepted. Remote URLs
/// are refused (see the module SSRF note).
pub fn decode_image_url(url: &str) -> Result<DecodedImage, ImageDecodeError> {
    let lower = url.trim_start().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Err(ImageDecodeError::RemoteUrlUnsupported);
    }
    if !lower.starts_with("data:") {
        return Err(ImageDecodeError::BadScheme);
    }

    // data:[<mediatype>][;base64],<data>  — we require base64.
    let after = &url["data:".len()..];
    let comma = after
        .find(',')
        .ok_or(ImageDecodeError::MalformedDataUrl("no comma separator"))?;
    let (meta, payload) = after.split_at(comma);
    let payload = &payload[1..]; // drop the comma
    if !meta.to_ascii_lowercase().contains("base64") {
        return Err(ImageDecodeError::MalformedDataUrl(
            "only ;base64 data URLs are supported",
        ));
    }

    // The base64 payload may include whitespace/newlines from some
    // clients; strip them before decode. Use the standard alphabet.
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| ImageDecodeError::Base64(e.to_string()))?;

    decode_encoded_image(&bytes)
}

/// Decode encoded (PNG/JPEG/…) image bytes into raw RGB, with the
/// decompression-bomb guards applied.
pub fn decode_encoded_image(bytes: &[u8]) -> Result<DecodedImage, ImageDecodeError> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(ImageDecodeError::TooLargeEncoded(
            bytes.len(),
            MAX_ENCODED_BYTES,
        ));
    }

    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader = reader
        .with_guessed_format()
        .map_err(|e| ImageDecodeError::Decode(e.to_string()))?;

    // Bomb guards: bound the decoded dimensions AND the total allocation
    // the decoder is allowed to request. `image::Limits` enforces both
    // during decode, so an image whose header claims huge dimensions
    // fails fast rather than allocating.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(MAX_ALLOC_BYTES);
    reader.limits(limits);

    let img = reader
        .decode()
        .map_err(|e| ImageDecodeError::Decode(e.to_string()))?;

    let rgb = img.to_rgb8();
    let (width, height) = (rgb.width(), rgb.height());
    let raw = rgb.into_raw();
    debug_assert_eq!(raw.len(), (width as usize) * (height as usize) * 3);
    Ok(DecodedImage {
        width,
        height,
        rgb: raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1x1 red PNG, base64. Generated once; stable.
    const RED_1X1_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    #[test]
    fn data_url_png_decodes_to_rgb() {
        let url = format!("data:image/png;base64,{RED_1X1_PNG_B64}");
        let d = decode_image_url(&url).expect("decode");
        assert_eq!(d.width, 1);
        assert_eq!(d.height, 1);
        assert_eq!(d.rgb.len(), 3);
        // Red pixel: R high, G/B low.
        assert!(d.rgb[0] > 200, "expected red, got {:?}", d.rgb);
    }

    #[test]
    fn remote_url_refused() {
        let e = decode_image_url("https://evil.example/x.png").unwrap_err();
        assert!(matches!(e, ImageDecodeError::RemoteUrlUnsupported));
        let e = decode_image_url("http://169.254.169.254/latest/meta-data/").unwrap_err();
        assert!(matches!(e, ImageDecodeError::RemoteUrlUnsupported));
    }

    #[test]
    fn non_base64_data_url_rejected() {
        let e = decode_image_url("data:image/png,notbase64").unwrap_err();
        assert!(matches!(e, ImageDecodeError::MalformedDataUrl(_)));
    }

    #[test]
    fn garbage_scheme_rejected() {
        let e = decode_image_url("ftp://host/x.png").unwrap_err();
        assert!(matches!(e, ImageDecodeError::BadScheme));
    }

    #[test]
    fn bad_base64_rejected() {
        let e = decode_image_url("data:image/png;base64,!!!!not-valid").unwrap_err();
        assert!(matches!(e, ImageDecodeError::Base64(_)));
    }

    #[test]
    fn non_image_bytes_fail_decode() {
        // Valid base64 of "hello" — not an image.
        let e = decode_image_url("data:image/png;base64,aGVsbG8=").unwrap_err();
        assert!(matches!(e, ImageDecodeError::Decode(_)));
    }
}
