//! Decode an OpenAI `input_audio` content part into mono little-endian
//! float32 PCM at the sample rate the daemon's backend requires.
//!
//! inferd's daemon links **no audio codec** (ADR 0016): an audio
//! attachment travels the wire as raw mono LE-f32 samples, and the
//! *consumer* decodes the source format. This bridge is that consumer for
//! OpenAI clients, so it decodes the wav/mp3 the client sends here.
//!
//! ## The sample rate is a hard contract, not a hint
//!
//! libmtmd's audio entry point takes no rate argument — the encoder
//! consumes samples at whatever rate it was trained for. Feeding 44.1 kHz
//! into a 16 kHz encoder time-scales the audio ~2.75x and returns a
//! *fluent wrong answer* that nothing in the bytes reveals. The daemon
//! therefore **rejects** any rate other than the one it advertises as
//! `audio_sample_rate` on its admin capabilities frame, and never
//! resamples (see `inferd-proto::v2::attachment`).
//!
//! Rate conversion is consequently the consumer's job, which is this
//! module's other half: whatever the client sent is resampled to the
//! advertised rate before it reaches the wire (ADR 0025). The bridge
//! *reads* that rate off the admin socket rather than hardcoding 16000 —
//! a different mmproj advertises a different rate.
//!
//! ## Security posture
//!
//! - **No SSRF surface.** OpenAI defines no `audio_url` form: audio is
//!   always inline base64. There is no URL to fetch, so unlike
//!   [`crate::image_decode`] there is nothing to guard here.
//! - **Decompression bombs.** A few-hundred-KB mp3 decodes to hundreds of
//!   MiB of f32. Three bounds apply: the encoded payload size, the
//!   accumulated decoded sample count (checked *during* the decode loop,
//!   so a bomb fails partway rather than after full expansion), and the
//!   resampled payload size (checked from the predicted length, before
//!   doing the resampling work).

use base64::Engine as _;
// `rubato` re-exports the buffer crate, so we don't take a direct
// dependency on a crate whose version is rubato's to choose.
use rubato::{FixedSync, Resampler, audioadapter_buffers};
use std::io::Cursor;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

/// Decoded audio ready to become an inferd attachment: mono f32 samples
/// at the rate the backend requires.
#[derive(Debug)]
pub struct DecodedAudio {
    /// Sample rate in Hz. Equals the `target_rate` requested of
    /// [`decode_input_audio`] — the whole point of this module.
    pub sample_rate: u32,
    /// Mono samples, nominally in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
}

impl DecodedAudio {
    /// Serialize to the wire form: mono little-endian float32 octets.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.samples.len() * 4);
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

/// Failures decoding an `input_audio` part → HTTP 400.
#[derive(Debug, thiserror::Error)]
pub enum AudioDecodeError {
    /// The base64 payload didn't decode.
    #[error("audio base64 decode failed: {0}")]
    Base64(String),
    /// The encoded clip exceeded the byte cap before decoding.
    #[error("audio too large: {0} bytes exceeds the {1}-byte cap")]
    TooLargeEncoded(usize, usize),
    /// The container/codec was unrecognised, or the stream was malformed.
    #[error("audio decode failed: {0}")]
    Decode(String),
    /// The stream carried no audio track (e.g. a video-only container).
    #[error("no audio track found in the supplied audio")]
    NoAudioTrack,
    /// The stream decoded to zero samples.
    #[error("audio decoded to zero samples")]
    Empty,
    /// The decoded sample count exceeded the cap (bomb guard).
    #[error("decoded audio exceeds the {0}-sample cap")]
    TooManySamples(usize),
    /// The resampled payload would exceed the wire byte cap.
    #[error("resampled audio would be {0} bytes, over the {1}-byte cap")]
    TooLargePcm(usize, usize),
    /// The resampler could not be built or failed mid-run.
    #[error("audio resample failed: {0}")]
    Resample(String),
}

/// Max encoded (compressed) audio bytes accepted per clip. The HTTP body
/// cap (8 MiB) bounds the whole request, and base64 inflates by 4/3, so
/// in practice a single inline clip cannot exceed ~6 MiB regardless —
/// this makes the intent explicit and bounds the decoder's input directly.
const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;

/// Max decoded mono samples accumulated during the decode loop. Rate- and
/// channel-count-agnostic (the downmix happens per packet, so the
/// accumulator is always mono), which is what makes one constant
/// sufficient: 16 Mi samples is 64 MiB of f32 held at peak. At the
/// daemon's 16 kHz that is ~17 minutes of audio — more than any speech
/// prompt — while a 200 KB mp3 that claims to decode to hours fails
/// partway through rather than after fully expanding.
const MAX_DECODED_SAMPLES: usize = 16 * 1024 * 1024;

/// Max resampled PCM payload, in octets. 48 MiB keeps one attachment
/// under the daemon's 64 MiB per-frame BLOB cap with headroom for the
/// JSON request frame — the same reasoning as `image_decode::MAX_DIM`.
/// Checked against the *predicted* output length, so an upsample that
/// would blow the cap is refused before the work is done.
const MAX_PCM_BYTES: usize = 48 * 1024 * 1024;

/// Resampler chunk size in frames. `rubato::Fft::new` derives its FFT
/// sub-chunk size from this (targeting ~256 frames per sub-chunk); 1024
/// gives four sub-chunks, which keeps the resampler's startup delay small
/// without pushing the anti-alias cutoff down.
const RESAMPLE_CHUNK: usize = 1024;

/// Decode a base64 `input_audio.data` payload into mono f32 PCM at
/// `target_rate`.
///
/// `format_hint` is the OpenAI `input_audio.format` field (`"wav"` /
/// `"mp3"`). It is passed to the prober as an extension hint only — the
/// real format is detected from the bytes, so a wrong hint costs nothing.
pub fn decode_input_audio(
    data: &str,
    format_hint: &str,
    target_rate: u32,
) -> Result<DecodedAudio, AudioDecodeError> {
    // Some clients wrap the base64 at column 76; strip whitespace before
    // decoding. Standard alphabet, per the OpenAI wire.
    let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| AudioDecodeError::Base64(e.to_string()))?;

    decode_encoded_audio(bytes, format_hint, target_rate)
}

/// Decode encoded (wav/mp3) audio bytes into mono f32 PCM at
/// `target_rate`, with the bomb guards applied.
pub fn decode_encoded_audio(
    bytes: Vec<u8>,
    format_hint: &str,
    target_rate: u32,
) -> Result<DecodedAudio, AudioDecodeError> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(AudioDecodeError::TooLargeEncoded(
            bytes.len(),
            MAX_ENCODED_BYTES,
        ));
    }

    let (source_rate, mono) = decode_to_mono(bytes, format_hint)?;
    if mono.is_empty() {
        return Err(AudioDecodeError::Empty);
    }

    resample_to(mono, source_rate, target_rate)
}

/// Demux + decode the clip, downmixing each packet to mono as it arrives.
///
/// Downmixing per packet rather than after the fact is deliberate: it
/// keeps the accumulator mono, so peak memory is independent of the
/// source's channel count and one sample cap bounds it.
fn decode_to_mono(bytes: Vec<u8>, format_hint: &str) -> Result<(u32, Vec<f32>), AudioDecodeError> {
    // `Cursor<Vec<u8>>` is a `MediaSource` (seekable, known length), so
    // the clip never touches the filesystem.
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());

    let mut hint = Hint::new();
    let ext: String = format_hint
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if !ext.is_empty() {
        hint.with_extension(&ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, Default::default(), Default::default())
        .map_err(|e| AudioDecodeError::Decode(e.to_string()))?;

    // Copy the track id + codec params out before borrowing `format`
    // mutably for the packet loop.
    let (track_id, params) = {
        let track = format
            .default_track(TrackType::Audio)
            .or_else(|| format.first_track_known_codec(TrackType::Audio))
            .ok_or(AudioDecodeError::NoAudioTrack)?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or(AudioDecodeError::NoAudioTrack)?
            .clone();
        (track.id, params)
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| AudioDecodeError::Decode(e.to_string()))?;

    let mut mono: Vec<f32> = Vec::new();
    // Interleaved scratch, reused across packets. `copy_to_vec_interleaved`
    // *resizes* the destination rather than appending, so it must not be
    // the accumulator.
    let mut scratch: Vec<f32> = Vec::new();
    let mut source_rate: Option<u32> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // Some readers signal end-of-stream as an unexpected EOF.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(AudioDecodeError::Decode(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A corrupt packet is recoverable: skip it and keep going,
            // which is what a player does and what makes a slightly
            // damaged upload still usable.
            Err(SymphoniaError::DecodeError(_)) => continue,
            // The stream asked for a decoder reset mid-clip. Everything
            // decoded so far is valid; stop there rather than silently
            // splicing a re-initialised stream onto it.
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(AudioDecodeError::Decode(e.to_string())),
        };

        let rate = decoded.spec().rate();
        let channels = decoded.spec().channels().count();
        if rate == 0 || channels == 0 {
            return Err(AudioDecodeError::Decode(
                "decoder reported a zero sample rate or channel count".into(),
            ));
        }
        // A mid-clip rate change would need a resampler restart; refuse it
        // rather than concatenate two rates into one buffer.
        match source_rate {
            None => source_rate = Some(rate),
            Some(r) if r != rate => {
                return Err(AudioDecodeError::Decode(format!(
                    "sample rate changes mid-stream ({r} Hz then {rate} Hz)"
                )));
            }
            Some(_) => {}
        }

        let frames = decoded.frames();
        if mono.len().saturating_add(frames) > MAX_DECODED_SAMPLES {
            return Err(AudioDecodeError::TooManySamples(MAX_DECODED_SAMPLES));
        }
        downmix_into(&decoded, channels, &mut scratch, &mut mono);
    }

    let rate = source_rate.ok_or(AudioDecodeError::Empty)?;
    Ok((rate, mono))
}

/// Append `decoded`, downmixed to mono, onto `mono`, using `scratch` as
/// the interleaved staging buffer.
fn downmix_into(
    decoded: &GenericAudioBufferRef<'_>,
    channels: usize,
    scratch: &mut Vec<f32>,
    mono: &mut Vec<f32>,
) {
    // Generic over the source sample format — symphonia converts to f32
    // for us, so there's no match over its ten buffer variants.
    decoded.copy_to_vec_interleaved(scratch);
    if channels == 1 {
        mono.extend_from_slice(scratch);
        return;
    }
    let inv = 1.0 / channels as f32;
    mono.extend(
        scratch
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * inv),
    );
}

/// Resample mono `samples` from `source_rate` to `target_rate`. A no-op
/// (beyond the byte cap check) when the rates already match — the common
/// case for a client that recorded at the advertised rate.
fn resample_to(
    samples: Vec<f32>,
    source_rate: u32,
    target_rate: u32,
) -> Result<DecodedAudio, AudioDecodeError> {
    // Predicted output length, so an upsample that would blow the wire cap
    // is refused before the resampling work happens.
    let predicted = (samples.len() as u128 * target_rate as u128 / source_rate.max(1) as u128)
        .min(usize::MAX as u128) as usize;
    let predicted_bytes = predicted.saturating_mul(4);
    if predicted_bytes > MAX_PCM_BYTES {
        return Err(AudioDecodeError::TooLargePcm(
            predicted_bytes,
            MAX_PCM_BYTES,
        ));
    }

    if source_rate == target_rate {
        return Ok(DecodedAudio {
            sample_rate: target_rate,
            samples,
        });
    }

    let mut resampler = rubato::Fft::<f32>::new(
        source_rate as usize,
        target_rate as usize,
        RESAMPLE_CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|e| AudioDecodeError::Resample(e.to_string()))?;

    let input = audioadapter_buffers::direct::InterleavedSlice::new(&samples[..], 1, samples.len())
        .map_err(|e| AudioDecodeError::Resample(e.to_string()))?;
    let out = resampler
        .process_all(&input, samples.len(), None)
        .map_err(|e| AudioDecodeError::Resample(e.to_string()))?;

    Ok(DecodedAudio {
        sample_rate: target_rate,
        samples: out.take_data(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal RIFF/WAVE container around 16-bit PCM samples.
    /// Hand-rolled so the tests exercise the real symphonia path without
    /// shipping a binary fixture.
    fn wav_pcm16(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        w.extend_from_slice(&(channels * 2).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn wav_at_target_rate_decodes_without_resampling() {
        let samples: Vec<i16> = (0..1600)
            .map(|n| ((n % 100) * 300 - 15000) as i16)
            .collect();
        let wav = wav_pcm16(16_000, 1, &samples);
        let d = decode_input_audio(&b64(&wav), "wav", 16_000).expect("decode");
        assert_eq!(d.sample_rate, 16_000);
        // No resampling, so the sample count is exact.
        assert_eq!(d.samples.len(), 1600);
        assert_eq!(d.to_le_bytes().len(), 1600 * 4);
    }

    #[test]
    fn stereo_is_downmixed_to_mono() {
        // Anti-phase channels: L = +peak, R = -peak → mono ≈ 0.
        let mut samples = Vec::new();
        for _ in 0..800 {
            samples.push(20_000i16);
            samples.push(-20_000i16);
        }
        let wav = wav_pcm16(16_000, 2, &samples);
        let d = decode_input_audio(&b64(&wav), "wav", 16_000).expect("decode");
        assert_eq!(d.samples.len(), 800, "one mono sample per stereo frame");
        for s in &d.samples {
            assert!(s.abs() < 0.01, "anti-phase downmix should cancel, got {s}");
        }
    }

    #[test]
    fn resamples_up_to_the_target_rate() {
        // 1 s of 8 kHz audio → ~1 s of 16 kHz audio.
        let samples: Vec<i16> = (0..8_000)
            .map(|n| ((n as f32 / 8_000.0 * 440.0 * std::f32::consts::TAU).sin() * 20_000.0) as i16)
            .collect();
        let wav = wav_pcm16(8_000, 1, &samples);
        let d = decode_input_audio(&b64(&wav), "wav", 16_000).expect("decode");
        assert_eq!(d.sample_rate, 16_000);
        let got = d.samples.len() as f64;
        assert!(
            (got - 16_000.0).abs() / 16_000.0 < 0.01,
            "expected ~16000 samples, got {got}"
        );
        // The resampled tone should still carry energy — a silent result
        // would mean the resampler emitted only its startup padding.
        let peak = d.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.3, "resampled signal lost its amplitude: {peak}");
    }

    #[test]
    fn resamples_down_to_the_target_rate() {
        let samples: Vec<i16> = (0..44_100)
            .map(|n| {
                ((n as f32 / 44_100.0 * 440.0 * std::f32::consts::TAU).sin() * 20_000.0) as i16
            })
            .collect();
        let wav = wav_pcm16(44_100, 1, &samples);
        let d = decode_input_audio(&b64(&wav), "wav", 16_000).expect("decode");
        let got = d.samples.len() as f64;
        assert!(
            (got - 16_000.0).abs() / 16_000.0 < 0.01,
            "expected ~16000 samples, got {got}"
        );
    }

    #[test]
    fn bad_base64_rejected() {
        let e = decode_input_audio("!!!not-valid", "wav", 16_000).unwrap_err();
        assert!(matches!(e, AudioDecodeError::Base64(_)), "got {e:?}");
    }

    #[test]
    fn non_audio_bytes_rejected() {
        // Valid base64 of "hello" — not audio.
        let e = decode_input_audio("aGVsbG8=", "wav", 16_000).unwrap_err();
        assert!(matches!(e, AudioDecodeError::Decode(_)), "got {e:?}");
    }

    #[test]
    fn zero_length_audio_rejected() {
        let wav = wav_pcm16(16_000, 1, &[]);
        let e = decode_input_audio(&b64(&wav), "wav", 16_000).unwrap_err();
        assert!(
            matches!(e, AudioDecodeError::Empty | AudioDecodeError::Decode(_)),
            "got {e:?}"
        );
    }

    #[test]
    fn oversized_encoded_payload_rejected() {
        let big = vec![0u8; MAX_ENCODED_BYTES + 1];
        let e = decode_encoded_audio(big, "wav", 16_000).unwrap_err();
        assert!(
            matches!(e, AudioDecodeError::TooLargeEncoded(_, _)),
            "got {e:?}"
        );
    }

    #[test]
    fn wrong_format_hint_still_decodes() {
        // The hint is advisory; detection is from the bytes.
        let samples: Vec<i16> = vec![1000; 320];
        let wav = wav_pcm16(16_000, 1, &samples);
        let d = decode_input_audio(&b64(&wav), "mp3", 16_000).expect("decode");
        assert_eq!(d.samples.len(), 320);
    }

    #[test]
    fn le_f32_serialization_round_trips() {
        let d = DecodedAudio {
            sample_rate: 16_000,
            samples: vec![0.0, 1.0, -0.5],
        };
        let bytes = d.to_le_bytes();
        assert_eq!(bytes.len(), 12);
        assert_eq!(&bytes[0..4], &0.0f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[8..12], &(-0.5f32).to_le_bytes());
    }
}
