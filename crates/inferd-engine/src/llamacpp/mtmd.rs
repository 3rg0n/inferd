//! Safe Rust wrapper over `libmtmd`.
//!
//! `libmtmd` is upstream's multimodal-tokenisation library: given a
//! prompt with `<__media__>` markers and a parallel slice of bitmaps
//! (image RGB or audio f32 PCM), it produces a sequence of "input
//! chunks" that the engine can splice into `llama_decode`'s input
//! batch. See `vendor/llama.cpp/tools/mtmd/mtmd.h` for the C API.
//!
//! This wrapper is thin: each safe type owns one C handle and frees
//! it on Drop. The wrapper does NOT do encode/decode (that's the
//! `LlamaCpp` adapter's job once Phase 3+ wires the per-chunk loop).
//! It just makes the FFI surface usable from safe Rust.
//!
//! ## Lifetime / threading
//!
//! - `Mtmd` owns a `*mut mtmd_context`. It borrows the parent
//!   [`crate::llamacpp::ModelHandle`] for its lifetime — the C
//!   context holds a non-owning pointer to the `llama_model` and
//!   the model must outlive the mtmd context. The Rust borrow makes
//!   that explicit.
//! - `Bitmap` owns a `*mut mtmd_bitmap`. Independent of `Mtmd`; it
//!   can be created before the context exists and reused across
//!   contexts.
//! - `InputChunks` owns a `*mut mtmd_input_chunks`. Lives across
//!   `tokenize` calls; the C API populates it.
//!
//! Per `mtmd.h`, `mtmd_tokenize` is documented as thread-safe with
//! a shared `ctx`. We don't yet expose that; v0.1's admission queue
//! serialises generations.

#![allow(unsafe_code)]

use crate::mtmd_ffi as ffi;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr::NonNull;

/// Errors from the safe `Mtmd` wrapper.
#[derive(Debug, thiserror::Error)]
pub enum MtmdError {
    /// `mtmd_init_from_file` returned null.
    #[error("mtmd_init_from_file returned null (mmproj load failed)")]
    InitFromFile,
    /// Path string contained an interior NUL.
    #[error("path contains NUL byte: {0}")]
    PathNul(std::path::PathBuf),
    /// Bitmap allocation failed.
    #[error("mtmd_bitmap_init returned null")]
    BitmapAlloc,
    /// `mtmd_tokenize` returned a non-zero error code:
    ///   1 → number of bitmaps did not match number of media markers
    ///   2 → image preprocessing error
    #[error("mtmd_tokenize failed: code={0}")]
    Tokenize(i32),
    /// `mtmd_encode_chunk` returned a non-zero error code.
    #[error("mtmd_encode_chunk failed: code={0}")]
    EncodeChunk(i32),
    /// Internal invariant violation.
    #[error("mtmd internal: {0}")]
    Internal(&'static str),
}

/// Capabilities reported by an mmproj file. Read at startup so the
/// daemon can advertise the right `BackendCapabilities` without
/// instantiating a full mtmd context.
#[derive(Debug, Clone, Copy, Default)]
pub struct MmprojCaps {
    /// `true` if the mmproj's vision projector is loadable.
    pub vision: bool,
    /// `true` if the mmproj's audio projector is loadable.
    pub audio: bool,
}

/// Probe an mmproj file for the modalities it supports without
/// instantiating a full `Mtmd`. Wraps `mtmd_get_cap_from_file`.
pub fn probe_mmproj_caps(mmproj: &Path) -> Result<MmprojCaps, MtmdError> {
    let cpath = path_to_cstring(mmproj)?;
    // SAFETY: `cpath` outlives the call; the C function only reads
    // through the pointer.
    let raw = unsafe { ffi::mtmd_get_cap_from_file(cpath.as_ptr()) };
    Ok(MmprojCaps {
        vision: raw.inp_vision,
        audio: raw.inp_audio,
    })
}

/// Multimodal context wrapping an `mtmd_context`. Borrows the parent
/// `llama_model` to ensure it outlives this handle.
pub struct Mtmd {
    ctx: NonNull<ffi::mtmd_context>,
}

// SAFETY: `mtmd_context` is opaque from Rust's view; per `mtmd.h`,
// `mtmd_tokenize` is thread-safe with a shared ctx. The other
// mutating ops (encode, free) are scoped to `&mut self`. v0.1's
// admission queue serialises generations regardless.
unsafe impl Send for Mtmd {}
unsafe impl Sync for Mtmd {}

/// Construction parameters for `Mtmd::new`.
#[derive(Debug, Clone)]
pub struct MtmdConfig {
    /// `true` to allow GPU offload of the projector layers when a
    /// GPU backend is available.
    pub use_gpu: bool,
    /// `true` to print mtmd's per-decode timing (for diagnostics).
    pub print_timings: bool,
    /// Threads to use for projector inference. `None` uses the C
    /// default (typically the active llama context's thread count).
    pub n_threads: Option<i32>,
    /// `true` to run a warmup encode after init. Catches setup
    /// errors at startup rather than first-request.
    pub warmup: bool,
    /// Maximum number of image tokens the projector may emit per image,
    /// for **dynamic-resolution** vision models (Gemma 4 is one). The
    /// projector downscales/tiles the input toward this budget before
    /// encoding; a higher cap means less downscaling, so small or
    /// sparsely-spaced text (OCR of leader-dotted lines, fine print)
    /// survives, at the cost of more tokens and slower encode. `None`
    /// reads the model's metadata default. Maps to
    /// `mtmd_context_params::image_max_tokens`. Set at init time (it's a
    /// context property, not per-request) — see issue #42.
    pub image_max_tokens: Option<i32>,
    /// Minimum image tokens per image (dynamic-resolution models). `None`
    /// reads the metadata default. Maps to
    /// `mtmd_context_params::image_min_tokens`. Rarely needed; exposed for
    /// symmetry with `image_max_tokens`.
    pub image_min_tokens: Option<i32>,
}

impl Default for MtmdConfig {
    fn default() -> Self {
        Self {
            use_gpu: true,
            print_timings: false,
            n_threads: None,
            warmup: true,
            image_max_tokens: None,
            image_min_tokens: None,
        }
    }
}

impl Mtmd {
    /// Initialise an mtmd context against the given mmproj file +
    /// already-loaded text model. The text model must outlive the
    /// returned `Mtmd` (enforced by the borrow on `crate::llamacpp::ModelHandle`).
    ///
    /// # Safety
    ///
    /// Caller guarantees the `llama_model` pointer is alive for at
    /// least the lifetime of the returned `Mtmd`. In practice this
    /// is enforced by the `LlamaCpp` adapter holding both handles
    /// in the same struct.
    pub unsafe fn new(
        mmproj: &Path,
        text_model: *const crate::ffi::llama_model,
        config: MtmdConfig,
    ) -> Result<Self, MtmdError> {
        let cpath = path_to_cstring(mmproj)?;
        // SAFETY: defaults from C; we mutate the fields we care about.
        let mut params = unsafe { ffi::mtmd_context_params_default() };
        params.use_gpu = config.use_gpu;
        params.print_timings = config.print_timings;
        if let Some(n) = config.n_threads {
            params.n_threads = n;
        }
        params.warmup = config.warmup;
        // Dynamic-resolution image-token budget (issue #42). Only set
        // when the operator overrode the metadata default; otherwise
        // leave the value `mtmd_context_params_default()` read from the
        // model so behaviour is unchanged.
        if let Some(n) = config.image_max_tokens {
            params.image_max_tokens = n;
        }
        if let Some(n) = config.image_min_tokens {
            params.image_min_tokens = n;
        }

        // SAFETY: `cpath` outlives the call. `text_model` outlives
        // the returned context per the function's documented
        // contract. The cast is safe — the llama_model pointer
        // bindgen produced for libllama and the one mtmd's bindings
        // expect both alias the same opaque struct from the same
        // `llama.h` header.
        let raw = unsafe { ffi::mtmd_init_from_file(cpath.as_ptr(), text_model.cast(), params) };
        let ctx = NonNull::new(raw).ok_or(MtmdError::InitFromFile)?;
        Ok(Self { ctx })
    }

    /// `true` if the loaded mmproj supports vision input.
    pub fn supports_vision(&self) -> bool {
        // SAFETY: `self.ctx` is non-null and owned.
        unsafe { ffi::mtmd_support_vision(self.ctx.as_ptr()) }
    }

    /// `true` if the loaded mmproj supports audio input.
    pub fn supports_audio(&self) -> bool {
        unsafe { ffi::mtmd_support_audio(self.ctx.as_ptr()) }
    }

    /// Audio sample rate the mmproj's audio encoder expects, in Hz.
    /// Returns `None` if the mmproj is vision-only.
    pub fn audio_sample_rate(&self) -> Option<u32> {
        let n = unsafe { ffi::mtmd_get_audio_sample_rate(self.ctx.as_ptr()) };
        if n <= 0 { None } else { Some(n as u32) }
    }

    /// Run the helper-driven evaluation loop over `chunks` against
    /// `lctx`. For each text chunk, runs `llama_decode`; for each
    /// image/audio chunk, runs `mtmd_encode_chunk` and splices the
    /// resulting embeddings into the next `llama_decode`. Forwards
    /// any non-zero internal error.
    ///
    /// Returns the new `n_past` (token position after this batch
    /// has been consumed). This is what the sampler loop should
    /// resume from when generating the response.
    ///
    /// # Safety
    ///
    /// Caller guarantees:
    ///   - `lctx` was created from the same `llama_model` this
    ///     `Mtmd` borrows.
    ///   - `chunks` was produced by `Mtmd::tokenize` against `self`
    ///     (using a different `Mtmd` is undefined behaviour even
    ///     though both wrap an `mtmd_context`).
    pub unsafe fn eval_chunks(
        &self,
        lctx: *mut crate::ffi::llama_context,
        chunks: &InputChunks,
        n_past: i32,
        seq_id: i32,
        n_batch: i32,
        logits_last: bool,
    ) -> Result<i32, MtmdError> {
        let mut new_n_past: i32 = 0;
        // SAFETY: pointers all valid for the call's duration. The
        // helper is documented as not thread-safe; v0.1's admission
        // queue serialises generations.
        let rc = unsafe {
            ffi::mtmd_helper_eval_chunks(
                self.ctx.as_ptr(),
                lctx.cast(),
                chunks.raw(),
                n_past,
                seq_id,
                n_batch,
                logits_last,
                &mut new_n_past,
            )
        };
        if rc != 0 {
            return Err(MtmdError::EncodeChunk(rc));
        }
        Ok(new_n_past)
    }

    /// Tokenise `text` (containing `<__media__>` markers) plus the
    /// matching ordered slice of bitmaps. Number of bitmaps must
    /// equal number of markers.
    pub fn tokenize(&self, text: &str, bitmaps: &[&Bitmap]) -> Result<InputChunks, MtmdError> {
        let c_text = CString::new(text).map_err(|_| MtmdError::Internal("text contains NUL"))?;
        let in_text = ffi::mtmd_input_text {
            text: c_text.as_ptr(),
            add_special: true,
            parse_special: true,
        };
        let mut bitmap_ptrs: Vec<*const ffi::mtmd_bitmap> =
            bitmaps.iter().map(|b| b.raw() as *const _).collect();

        let chunks = InputChunks::new()?;

        // SAFETY: ctx, in_text, bitmap_ptrs all outlive the call. The
        // C function fills `chunks` with new owned chunks; we
        // transferred ownership of `chunks` into `InputChunks` already.
        // The `*mut *const T` in the binding is C's mutable-array-of-
        // const-pointers idiom; in practice the C function does not
        // mutate the array, but the binding's signature requires a
        // mut pointer.
        let rc = unsafe {
            ffi::mtmd_tokenize(
                self.ctx.as_ptr(),
                chunks.raw(),
                &in_text,
                bitmap_ptrs.as_mut_ptr(),
                bitmap_ptrs.len(),
            )
        };
        if rc != 0 {
            return Err(MtmdError::Tokenize(rc));
        }
        Ok(chunks)
    }
}

impl Drop for Mtmd {
    fn drop(&mut self) {
        // SAFETY: ctx is non-null and owned; Drop runs once.
        unsafe { ffi::mtmd_free(self.ctx.as_ptr()) };
    }
}

/// Owned mtmd_bitmap. Holds either an image (RGB) or audio (f32 PCM)
/// payload.
pub struct Bitmap {
    ptr: NonNull<ffi::mtmd_bitmap>,
}

unsafe impl Send for Bitmap {}
unsafe impl Sync for Bitmap {}

impl Bitmap {
    /// Build an image bitmap from `width * height * 3` interleaved
    /// RGB octets. Caller is responsible for guaranteeing the slice
    /// is exactly that size.
    pub fn from_image_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<Self, MtmdError> {
        let expected = (width as usize) * (height as usize) * 3;
        if rgb.len() != expected {
            return Err(MtmdError::Internal("rgb slice length != width*height*3"));
        }
        // SAFETY: pointer is valid for `expected` bytes per the
        // length check; mtmd_bitmap_init copies internally.
        let raw = unsafe { ffi::mtmd_bitmap_init(width, height, rgb.as_ptr()) };
        let ptr = NonNull::new(raw).ok_or(MtmdError::BitmapAlloc)?;
        Ok(Self { ptr })
    }

    /// Build an audio bitmap from float32 PCM samples.
    pub fn from_audio_f32(samples: &[f32]) -> Result<Self, MtmdError> {
        // SAFETY: pointer is valid for `samples.len()` f32s; mtmd
        // copies internally.
        let raw = unsafe { ffi::mtmd_bitmap_init_from_audio(samples.len(), samples.as_ptr()) };
        let ptr = NonNull::new(raw).ok_or(MtmdError::BitmapAlloc)?;
        Ok(Self { ptr })
    }

    /// `true` if this bitmap carries audio rather than image data.
    pub fn is_audio(&self) -> bool {
        unsafe { ffi::mtmd_bitmap_is_audio(self.ptr.as_ptr()) }
    }

    /// Set an optional ID on the bitmap. Used by upstream for KV
    /// cache de-duplication when the same image appears multiple
    /// times in a conversation.
    pub fn set_id(&mut self, id: &str) -> Result<(), MtmdError> {
        let cid = CString::new(id).map_err(|_| MtmdError::Internal("id contains NUL"))?;
        unsafe { ffi::mtmd_bitmap_set_id(self.ptr.as_ptr(), cid.as_ptr()) };
        Ok(())
    }

    /// Borrow the inner pointer for FFI calls. `pub(crate)` so
    /// sibling modules in the crate (the LlamaCpp adapter) can pass
    /// it to mtmd_tokenize.
    pub(crate) fn raw(&self) -> *mut ffi::mtmd_bitmap {
        self.ptr.as_ptr()
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { ffi::mtmd_bitmap_free(self.ptr.as_ptr()) };
    }
}

/// Owned `mtmd_input_chunks` collection. Populated by `Mtmd::tokenize`.
pub struct InputChunks {
    ptr: NonNull<ffi::mtmd_input_chunks>,
}

unsafe impl Send for InputChunks {}
unsafe impl Sync for InputChunks {}

impl InputChunks {
    fn new() -> Result<Self, MtmdError> {
        // SAFETY: returns owned heap allocation per docs.
        let raw = unsafe { ffi::mtmd_input_chunks_init() };
        let ptr = NonNull::new(raw).ok_or(MtmdError::Internal("mtmd_input_chunks_init"))?;
        Ok(Self { ptr })
    }

    /// Number of chunks the tokenizer produced.
    pub fn len(&self) -> usize {
        unsafe { ffi::mtmd_input_chunks_size(self.ptr.as_ptr()) }
    }

    /// `true` if the chunk list is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Borrow the chunk at index `i`. Returns `None` if out of bounds.
    pub fn get(&self, i: usize) -> Option<InputChunk<'_>> {
        if i >= self.len() {
            return None;
        }
        let raw = unsafe { ffi::mtmd_input_chunks_get(self.ptr.as_ptr(), i) };
        NonNull::new(raw as *mut _).map(|ptr| InputChunk {
            ptr,
            _marker: std::marker::PhantomData,
        })
    }

    pub(crate) fn raw(&self) -> *mut ffi::mtmd_input_chunks {
        self.ptr.as_ptr()
    }
}

impl Drop for InputChunks {
    fn drop(&mut self) {
        unsafe { ffi::mtmd_input_chunks_free(self.ptr.as_ptr()) };
    }
}

/// Borrow of one chunk inside an `InputChunks` collection. The
/// `_marker` lifetime ties the borrow to the parent collection so
/// the chunk pointer can't outlive its container.
pub struct InputChunk<'a> {
    ptr: NonNull<ffi::mtmd_input_chunk>,
    _marker: std::marker::PhantomData<&'a InputChunks>,
}

/// Discriminated kind of an input chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChunkKind {
    /// Plain text tokens. Splice into `llama_decode` input directly.
    Text,
    /// Image embeddings. Run through `Mtmd::encode_chunk` to fill
    /// the projector embedding buffer, then splice that into
    /// `llama_decode`.
    Image,
    /// Audio embeddings. Same flow as image.
    Audio,
}

impl InputChunk<'_> {
    /// Kind discriminator.
    pub fn kind(&self) -> InputChunkKind {
        let raw = unsafe { ffi::mtmd_input_chunk_get_type(self.ptr.as_ptr()) };
        match raw {
            ffi::MTMD_INPUT_CHUNK_TYPE_TEXT => InputChunkKind::Text,
            ffi::MTMD_INPUT_CHUNK_TYPE_IMAGE => InputChunkKind::Image,
            ffi::MTMD_INPUT_CHUNK_TYPE_AUDIO => InputChunkKind::Audio,
            // Bindgen produces an i32; future variants land here. Treat
            // unknown as text so the engine doesn't error out, and let
            // the encode/splice path catch the mismatch loudly.
            _ => InputChunkKind::Text,
        }
    }

    /// Number of llama tokens this chunk contributes.
    pub fn n_tokens(&self) -> usize {
        unsafe { ffi::mtmd_input_chunk_get_n_tokens(self.ptr.as_ptr()) }
    }

    /// Number of temporal positions the chunk consumes (matters for
    /// M-RoPE models — see mtmd.h).
    pub fn n_pos(&self) -> i32 {
        unsafe { ffi::mtmd_input_chunk_get_n_pos(self.ptr.as_ptr()) }
    }

    /// Borrow the inner pointer for FFI calls. Used by the
    /// `LlamaCpp` adapter's encode loop in later phases.
    #[allow(dead_code)]
    pub(crate) fn raw(&self) -> *const ffi::mtmd_input_chunk {
        self.ptr.as_ptr()
    }

    /// Optional id (only present on image/audio chunks; text returns
    /// nullptr in C).
    pub fn id(&self) -> Option<&str> {
        let raw = unsafe { ffi::mtmd_input_chunk_get_id(self.ptr.as_ptr()) };
        if raw.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(raw).to_str().ok() }
    }
}

fn path_to_cstring(p: &Path) -> Result<CString, MtmdError> {
    CString::new(p.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| MtmdError::PathNul(p.to_path_buf()))
}

/// The default media marker the daemon's chat-template renderer
/// emits. Public so tests can compare against the same constant
/// without needing to call into mtmd.
pub fn default_media_marker() -> &'static str {
    // mtmd::default_media_marker() returns the literal "<__media__>"
    // — confirmed in tools/mtmd/mtmd.cpp:109. We hard-code it here
    // (constant in upstream's API) rather than calling through FFI
    // every time. The token is also referenced from
    // `crates/inferd-daemon/src/chat_template/gemma4.rs`.
    "<__media__>"
}
