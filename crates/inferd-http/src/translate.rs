//! OpenAI wire ⇄ inferd wire translation.
//!
//! Request direction: OpenAI JSON → inferd `RequestV2` / `EmbedRequest`.
//! Response direction: inferd `ResponseV2` stream → OpenAI SSE chunks;
//! `EmbedResponse` → OpenAI embeddings JSON.
//!
//! This mirrors the OUTBOUND mapping in
//! `inferd-engine/src/openai_compat/mapper.rs` (which goes the other
//! way). Both share the wire structs in `inferd-openai-wire`.

use inferd_openai_wire::{
    ChatChunk, ChatRequest, ChunkChoice, ChunkDelta, ChunkToolCallDelta,
    ChunkToolCallFunctionDelta, ChunkUsage, ContentPart, EmbeddingData, EmbeddingVector,
    EmbeddingsRequest, EmbeddingsResponse, EmbeddingsUsage, MessageContent,
    ResponseFormat as OpenAiResponseFormat, ToolChoice as OpenAiToolChoice, ToolChoiceMode,
};
use inferd_proto::embed::EmbedRequest;
use inferd_proto::v2::{
    Attachment, ContentBlock, MessageV2, RequestV2, ResponseBlock, ResponseFormat, ResponseV2,
    RoleV2, StopReasonV2, Tool, ToolCallId, ToolChoice, UsageV2,
};
use thiserror::Error;

use crate::audio_decode::{self, AudioDecodeError};
use crate::image_decode::{self, ImageDecodeError};

/// Request-translation failures → HTTP 400.
#[derive(Debug, Error)]
pub enum TranslateError {
    /// `n > 1` — inferd emits a single choice per request.
    #[error("`n` > 1 is not supported (inferd generates one choice per request)")]
    MultipleChoices,
    /// Unknown chat role.
    #[error("unsupported role: {0}")]
    BadRole(String),
    /// Unknown `encoding_format` (only `float` and `base64` are valid).
    #[error("unsupported encoding_format: {0} (expected `float` or `base64`)")]
    BadEncodingFormat(String),
    /// Embeddings `input` was neither a string nor an array of strings.
    #[error("embeddings `input` must be a string or an array of strings")]
    BadEmbedInput,
    /// A tool-call's `arguments` string wasn't valid JSON.
    #[error("tool_call arguments were not valid JSON: {0}")]
    BadToolArgs(String),
    /// An `image_url` content part could not be decoded to RGB.
    #[error("image content: {0}")]
    Image(#[from] ImageDecodeError),
    /// An `image_url` part appeared on a non-user message. Only user
    /// turns carry images (the model produces text, and system prompts
    /// are text); an image on system/assistant/tool is a client error.
    #[error("image content is only allowed on `user` messages, not `{0}`")]
    ImageOnNonUser(String),
    /// The request carried more than [`MAX_IMAGES_PER_REQUEST`] images.
    #[error("too many images in one request (max {0})")]
    TooManyImages(usize),
    /// The decoded attachments exceeded the aggregate byte budget. Bounds
    /// the bridge's peak media memory: many small-compressed /
    /// large-decoded images or audio clips (each individually legal) can
    /// otherwise sum to gigabytes.
    #[error("decoded attachment bytes exceed the {0}-byte per-request budget")]
    AttachmentBudgetExceeded(usize),
    /// An `input_audio` content part could not be decoded to PCM.
    #[error("audio content: {0}")]
    Audio(#[from] AudioDecodeError),
    /// An `input_audio` part appeared on a non-user message. Same
    /// reasoning as [`TranslateError::ImageOnNonUser`].
    #[error("audio content is only allowed on `user` messages, not `{0}`")]
    AudioOnNonUser(String),
    /// The request carried more than [`MAX_AUDIO_CLIPS_PER_REQUEST`] clips.
    #[error("too many audio clips in one request (max {0})")]
    TooManyAudioClips(usize),
    /// The request carried audio but no registered backend takes audio, so
    /// there is no rate to resample to and the daemon would reject a guess
    /// (see [`crate::audio_decode`]). The narrower case — a backend that
    /// takes audio but advertises no rate, i.e. a daemon older than the
    /// field — is caught upstream in `handlers`, which can tell the two
    /// apart and says "upgrade the daemon" instead.
    #[error("the daemon's active backend does not accept audio input")]
    AudioUnsupported,
    /// `tool_choice` named a specific function
    /// (`{"type":"function","function":{"name":"…"}}`). The daemon's wire
    /// has no way to say "this tool and no other", so accepting it as
    /// `required` would let the model call a different declared tool
    /// while the caller believed it had pinned one. Rejected rather than
    /// widened — the caller can achieve the same thing by declaring only
    /// that tool with `tool_choice: "required"`.
    #[error(
        "tool_choice naming a specific function is not supported; \
         send `required` with only that tool declared"
    )]
    NamedToolChoice(String),
    /// `tool_choice` was an unrecognised string.
    #[error("unsupported tool_choice (expected `auto`, `required` or `none`)")]
    BadToolChoice,
    /// `tool_choice` arrived without `tools`. The daemon rejects this
    /// too, but the bridge says so with a 400 rather than relaying an
    /// IPC error, since it is a client mistake with a clear fix.
    #[error("tool_choice requires a non-empty `tools` array")]
    ToolChoiceWithoutTools,
}

/// Max number of image parts accepted in one chat request. A single
/// visual question rarely needs more than a handful of frames; a large
/// count is almost always abuse, and each image is a full RGB buffer the
/// bridge holds until it forwards the request.
pub const MAX_IMAGES_PER_REQUEST: usize = 8;

/// Max number of `input_audio` parts accepted in one chat request. Same
/// reasoning as [`MAX_IMAGES_PER_REQUEST`]; audio is bounded separately
/// because a clip's decoded size is unrelated to an image's.
pub const MAX_AUDIO_CLIPS_PER_REQUEST: usize = 4;

/// Aggregate decoded-media budget across **all** attachments (images and
/// audio) in one request. Caps the bridge's peak memory regardless of how
/// the compressed payloads packed into the 8 MiB HTTP body — a
/// decompression-amplification guard the per-item limits alone do not
/// provide. It is one shared budget rather than one per modality because
/// the daemon's own `MAX_ATTACHMENT_BYTES_PER_REQUEST` is also aggregate:
/// two independent 128 MiB budgets would let the bridge build a request
/// the daemon then refuses. 128 MiB comfortably fits several
/// full-resolution images or minutes of audio while refusing a bomb.
pub const MAX_TOTAL_DECODED_ATTACHMENT_BYTES: usize = 128 * 1024 * 1024;

// ===================== Chat request → RequestV2 =====================

/// Push a text content block, skipping empty strings (an empty text
/// block is noise the daemon would otherwise have to filter).
fn push_text(content: &mut Vec<ContentBlock>, text: String) {
    if !text.is_empty() {
        content.push(ContentBlock::Text { text });
    }
}

/// Map an OpenAI `response_format` to the daemon's structured-output
/// constraint. Only `json_schema` with a concrete schema maps to a
/// grammar; `text`/`json_object` (no schema to constrain against) and
/// unknown/absent forms yield `None` (unconstrained decoding).
fn map_response_format(rf: Option<OpenAiResponseFormat>) -> Option<ResponseFormat> {
    match rf {
        Some(OpenAiResponseFormat::JsonSchema { json_schema }) => json_schema
            .schema
            .map(|schema| ResponseFormat::JsonSchema { schema }),
        _ => None,
    }
}

/// Map an OpenAI `tool_choice` to the daemon's.
///
/// Unlike `response_format`, an unmappable value is an **error**, not a
/// silent `None`: dropping `required` hands the caller a guarantee the
/// daemon was never asked for. `has_tools` gates the field the same way
/// `RequestV2::resolve` does, reported here as a 400 rather than relayed
/// from IPC.
fn map_tool_choice(
    tc: Option<OpenAiToolChoice>,
    has_tools: bool,
) -> Result<Option<ToolChoice>, TranslateError> {
    let Some(tc) = tc else {
        return Ok(None);
    };
    if !has_tools {
        return Err(TranslateError::ToolChoiceWithoutTools);
    }
    match tc {
        OpenAiToolChoice::Mode(ToolChoiceMode::Auto) => Ok(Some(ToolChoice::Auto)),
        OpenAiToolChoice::Mode(ToolChoiceMode::Required) => Ok(Some(ToolChoice::Required)),
        OpenAiToolChoice::Mode(ToolChoiceMode::None) => Ok(Some(ToolChoice::None)),
        OpenAiToolChoice::Mode(ToolChoiceMode::Other) => Err(TranslateError::BadToolChoice),
        OpenAiToolChoice::Named(n) => Err(TranslateError::NamedToolChoice(n.function.name)),
    }
}

/// Translate an inbound OpenAI chat request into an inferd `RequestV2`.
///
/// `audio_rate` is the sample rate the daemon's active backend requires,
/// as read off its admin capabilities frame. Audio parts are resampled to
/// it (the daemon never resamples and rejects any other rate — see
/// [`crate::audio_decode`]). `None` means the backend takes no audio, so a
/// request carrying `input_audio` is rejected rather than sent at a
/// guessed rate.
pub fn chat_request_to_v2(
    req: ChatRequest,
    id: String,
    audio_rate: Option<u32>,
) -> Result<RequestV2, TranslateError> {
    if req.n.unwrap_or(1) > 1 {
        return Err(TranslateError::MultipleChoices);
    }
    let response_format = map_response_format(req.response_format);

    let mut messages = Vec::with_capacity(req.messages.len());
    // Media bytes ride out-of-band as attachments referenced by id; the
    // daemon reassembles them into BLOB frames. Ids are unique across the
    // whole request (`img-<n>` / `aud-<n>`), assigned per modality as
    // parts are encountered — the counters are per-modality so an id stays
    // contiguous within its kind regardless of interleaving.
    let mut attachments: Vec<Attachment> = Vec::new();
    let mut image_count: usize = 0;
    let mut audio_count: usize = 0;
    // Running total of decoded media bytes across the whole request,
    // checked against the aggregate budget after each decode.
    let mut total_media_bytes: usize = 0;
    for m in req.messages {
        let role = match m.role.as_str() {
            "system" => RoleV2::System,
            "user" | "tool" => RoleV2::User,
            "assistant" => RoleV2::Assistant,
            other => return Err(TranslateError::BadRole(other.to_string())),
        };

        let mut content: Vec<ContentBlock> = Vec::new();

        // A `role: "tool"` message is a tool RESULT addressed by id. Its
        // content is text only (tool outputs are text on the OpenAI wire).
        if m.role == "tool"
            && let Some(id) = m.tool_call_id
        {
            let inner = match m.content {
                Some(MessageContent::Text(t)) => vec![ContentBlock::Text { text: t }],
                Some(MessageContent::Parts(parts)) => {
                    // Concatenate any text parts; a tool result should not
                    // carry an image, so reject one rather than drop it.
                    let mut text = String::new();
                    for p in parts {
                        match p {
                            ContentPart::Text { text: t } => text.push_str(&t),
                            ContentPart::ImageUrl { .. } => {
                                return Err(TranslateError::ImageOnNonUser("tool".into()));
                            }
                            ContentPart::InputAudio { .. } => {
                                return Err(TranslateError::AudioOnNonUser("tool".into()));
                            }
                            ContentPart::Unknown => {}
                        }
                    }
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![ContentBlock::Text { text }]
                    }
                }
                None => Vec::new(),
            };
            content.push(ContentBlock::ToolResult {
                tool_call_id: ToolCallId::from(id),
                content: inner,
            });
            messages.push(MessageV2 { role, content });
            continue;
        }

        // Primary content: string or an array of typed parts.
        match m.content {
            Some(MessageContent::Text(text)) => push_text(&mut content, text),
            Some(MessageContent::Parts(parts)) => {
                for part in parts {
                    match part {
                        ContentPart::Text { text } => push_text(&mut content, text),
                        ContentPart::ImageUrl { image_url } => {
                            // Images are only meaningful on a user turn.
                            if role != RoleV2::User {
                                return Err(TranslateError::ImageOnNonUser(m.role.clone()));
                            }
                            // Bound the image COUNT before decoding the next
                            // one — refuse abuse without doing its work.
                            if image_count >= MAX_IMAGES_PER_REQUEST {
                                return Err(TranslateError::TooManyImages(MAX_IMAGES_PER_REQUEST));
                            }
                            let decoded = image_decode::decode_image_url(&image_url.url)?;
                            // Aggregate-byte guard: many individually-legal
                            // images can still sum to gigabytes of retained
                            // RGB (a compression-amplification DoS). Cap the
                            // running total.
                            total_media_bytes = total_media_bytes.saturating_add(decoded.rgb.len());
                            if total_media_bytes > MAX_TOTAL_DECODED_ATTACHMENT_BYTES {
                                return Err(TranslateError::AttachmentBudgetExceeded(
                                    MAX_TOTAL_DECODED_ATTACHMENT_BYTES,
                                ));
                            }
                            let id = format!("img-{image_count}");
                            image_count += 1;
                            content.push(ContentBlock::Image {
                                attachment_id: id.clone(),
                            });
                            let mut att = Attachment::Image {
                                id,
                                width: decoded.width,
                                height: decoded.height,
                                bytes: Vec::new(),
                            };
                            att.set_bytes(decoded.rgb);
                            attachments.push(att);
                        }
                        ContentPart::InputAudio { input_audio } => {
                            // Audio, like images, is only meaningful on a
                            // user turn.
                            if role != RoleV2::User {
                                return Err(TranslateError::AudioOnNonUser(m.role.clone()));
                            }
                            // The rate is the backend's, not ours to pick.
                            // No advertised rate → the daemon takes no
                            // audio, so fail here rather than build a
                            // request it will reject.
                            let target_rate = audio_rate.ok_or(TranslateError::AudioUnsupported)?;
                            if audio_count >= MAX_AUDIO_CLIPS_PER_REQUEST {
                                return Err(TranslateError::TooManyAudioClips(
                                    MAX_AUDIO_CLIPS_PER_REQUEST,
                                ));
                            }
                            let decoded = audio_decode::decode_input_audio(
                                &input_audio.data,
                                &input_audio.format,
                                target_rate,
                            )?;
                            let pcm = decoded.to_le_bytes();
                            total_media_bytes = total_media_bytes.saturating_add(pcm.len());
                            if total_media_bytes > MAX_TOTAL_DECODED_ATTACHMENT_BYTES {
                                return Err(TranslateError::AttachmentBudgetExceeded(
                                    MAX_TOTAL_DECODED_ATTACHMENT_BYTES,
                                ));
                            }
                            let id = format!("aud-{audio_count}");
                            audio_count += 1;
                            content.push(ContentBlock::Audio {
                                attachment_id: id.clone(),
                            });
                            let mut att = Attachment::Audio {
                                id,
                                sample_rate: decoded.sample_rate,
                                bytes: Vec::new(),
                            };
                            att.set_bytes(pcm);
                            attachments.push(att);
                        }
                        ContentPart::Unknown => {
                            // Ignore an unrecognised part type rather than
                            // fail — forward-compat with newer client fields.
                        }
                    }
                }
            }
            None => {}
        }

        // Assistant tool calls being replayed as history.
        for tc in m.tool_calls {
            let input = serde_json::from_str(&tc.function.arguments)
                .map_err(|e| TranslateError::BadToolArgs(e.to_string()))?;
            content.push(ContentBlock::ToolUse {
                tool_call_id: ToolCallId::from(tc.id),
                name: tc.function.name,
                input,
            });
        }

        // Skip a wholly-empty message (e.g. assistant turn with only a
        // now-consumed tool call) rather than send an empty content vec.
        if content.is_empty() {
            continue;
        }
        messages.push(MessageV2 { role, content });
    }

    let tools: Vec<Tool> = req
        .tools
        .into_iter()
        .map(|t| Tool {
            name: t.function.name,
            description: t.function.description,
            input_schema: t.function.parameters,
        })
        .collect();
    let tool_choice = map_tool_choice(req.tool_choice, !tools.is_empty())?;

    Ok(RequestV2 {
        id,
        messages,
        attachments,
        tools,
        tool_choice,
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens: req.max_tokens,
        stream: Some(req.stream),
        response_format,
        ..Default::default()
    })
}

// ===================== ResponseV2 → OpenAI chunks ===================

/// Accumulates inferd response frames and yields OpenAI chunk objects.
/// Text deltas map 1:1 to content chunks; tool-call blocks (which arrive
/// whole from the daemon) are buffered and emitted in the terminal
/// chunk; thinking deltas are dropped (no public OpenAI channel).
#[derive(Default)]
pub struct ChunkBuilder {
    id: String,
    model: String,
    created: u64,
    tool_calls: Vec<(String, String, String)>, // (id, name, args-json)
    first: bool,
}

impl ChunkBuilder {
    /// `created` is a Unix timestamp supplied by the caller (the crate
    /// cannot read the clock in a way that would break replay; the
    /// handler stamps it once).
    pub fn new(id: String, model: String, created: u64) -> Self {
        Self {
            id,
            model,
            created,
            tool_calls: Vec::new(),
            first: true,
        }
    }

    fn base_chunk(&self, streaming: bool) -> ChatChunk {
        ChatChunk {
            id: Some(self.id.clone()),
            object: Some(
                if streaming {
                    "chat.completion.chunk"
                } else {
                    "chat.completion"
                }
                .to_string(),
            ),
            created: Some(self.created),
            model: Some(self.model.clone()),
            choices: Vec::new(),
            usage: None,
        }
    }

    /// Ingest one frame. Returns `Some(chunk)` to stream, `None` when the
    /// frame only updates internal state (tool-call buffering, thinking).
    /// The terminal `Done`/`Error` is handled by [`Self::finalize`] /
    /// [`Self::error_from`].
    pub fn ingest(&mut self, frame: &ResponseV2) -> Option<ChatChunk> {
        match frame {
            ResponseV2::Frame { block, .. } => match block {
                ResponseBlock::Text { delta } => {
                    let role = if self.first {
                        self.first = false;
                        Some("assistant".to_string())
                    } else {
                        None
                    };
                    let mut chunk = self.base_chunk(true);
                    chunk.choices.push(ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role,
                            content: Some(delta.clone()),
                            tool_calls: Vec::new(),
                        },
                        finish_reason: None,
                    });
                    Some(chunk)
                }
                ResponseBlock::ToolUse {
                    tool_call_id,
                    name,
                    input,
                } => {
                    self.tool_calls.push((
                        tool_call_id.as_str().to_string(),
                        name.clone(),
                        serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                    ));
                    None
                }
                ResponseBlock::Thinking { .. } => None,
            },
            // Terminal frames are handled by finalize/error_from.
            _ => None,
        }
    }

    /// Build the terminal chunk for a successful `Done` frame:
    /// finish_reason + any buffered tool calls + usage.
    pub fn finalize(&self, usage: &UsageV2, stop: StopReasonV2) -> ChatChunk {
        let mut delta = ChunkDelta::default();
        for (i, (id, name, args)) in self.tool_calls.iter().enumerate() {
            delta.tool_calls.push(ChunkToolCallDelta {
                index: i,
                id: Some(id.clone()),
                kind: Some("function".to_string()),
                function: Some(ChunkToolCallFunctionDelta {
                    name: Some(name.clone()),
                    arguments: Some(args.clone()),
                }),
            });
        }
        let mut chunk = self.base_chunk(true);
        chunk.choices.push(ChunkChoice {
            index: 0,
            delta,
            finish_reason: Some(stop_reason_to_openai(stop).to_string()),
        });
        chunk.usage = Some(ChunkUsage {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
        });
        chunk
    }
}

/// Map inferd stop reasons to OpenAI `finish_reason`.
pub fn stop_reason_to_openai(stop: StopReasonV2) -> &'static str {
    match stop {
        StopReasonV2::EndTurn => "stop",
        StopReasonV2::MaxTokens => "length",
        StopReasonV2::ToolUse => "tool_calls",
        StopReasonV2::StopSequence => "stop",
        StopReasonV2::Cancelled => "stop",
        StopReasonV2::Error => "stop",
    }
}

// ===================== Embeddings ===================================

/// Requested embedding encoding. The OpenAI SDK defaults to `Base64`;
/// omitted means `Float`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingFormat {
    /// JSON float array.
    Float,
    /// base64 of little-endian f32 bytes (SDK default).
    Base64,
}

/// Translate an OpenAI embeddings request into an inferd `EmbedRequest`,
/// returning the requested encoding so the response can match it.
pub fn embeddings_request_to_inferd(
    req: EmbeddingsRequest,
    id: String,
) -> Result<(EmbedRequest, EncodingFormat), TranslateError> {
    let encoding = match req.encoding_format.as_deref() {
        None | Some("float") => EncodingFormat::Float,
        Some("base64") => EncodingFormat::Base64,
        Some(other) => return Err(TranslateError::BadEncodingFormat(other.to_string())),
    };
    let input: Vec<String> = match req.input {
        serde_json::Value::String(s) => vec![s],
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .map(|v| match v {
                serde_json::Value::String(s) => Ok(s),
                _ => Err(TranslateError::BadEmbedInput),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(TranslateError::BadEmbedInput),
    };
    Ok((
        EmbedRequest {
            id,
            input,
            dimensions: req.dimensions,
            task: None,
        },
        encoding,
    ))
}

/// Encode one f32 vector per the requested format.
fn encode_vector(v: Vec<f32>, fmt: EncodingFormat) -> EmbeddingVector {
    match fmt {
        EncodingFormat::Float => EmbeddingVector::Floats(v),
        EncodingFormat::Base64 => {
            // OpenAI encodes the raw little-endian f32 bytes as base64.
            let mut bytes = Vec::with_capacity(v.len() * 4);
            for f in &v {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            EmbeddingVector::Base64(base64_encode(&bytes))
        }
    }
}

/// Build an OpenAI embeddings response, encoding vectors per `fmt`.
pub fn embeddings_response_to_openai(
    embeddings: Vec<Vec<f32>>,
    model: String,
    prompt_tokens: u32,
    fmt: EncodingFormat,
) -> EmbeddingsResponse {
    let data = embeddings
        .into_iter()
        .enumerate()
        .map(|(index, embedding)| EmbeddingData {
            object: "embedding".to_string(),
            index,
            embedding: encode_vector(embedding, fmt),
        })
        .collect();
    EmbeddingsResponse {
        object: "list".to_string(),
        data,
        model,
        usage: EmbeddingsUsage {
            prompt_tokens,
            total_tokens: prompt_tokens,
        },
    }
}

/// Standard base64 (no line breaks), matching OpenAI's embedding
/// encoding. Small self-contained encoder to avoid a crate dep here.
fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use inferd_openai_wire::{
        ChatMessage, NamedToolChoice, NamedToolChoiceFunction, ToolCallFunction, ToolCallReplay,
        ToolDecl, ToolDeclFunction,
    };

    fn msg(role: &str, text: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: Some(MessageContent::Text(text.into())),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }
    }

    #[test]
    fn basic_chat_request_maps() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![msg("system", "be terse"), msg("user", "hi")],
            stream: true,
            temperature: Some(0.7),
            top_p: None,
            max_tokens: Some(64),
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "id1".into(), None).unwrap();
        assert_eq!(v2.messages.len(), 2);
        assert!(matches!(v2.messages[0].role, RoleV2::System));
        assert!(matches!(v2.messages[1].role, RoleV2::User));
        assert_eq!(v2.temperature, Some(0.7));
        assert_eq!(v2.max_tokens, Some(64));
    }

    #[test]
    fn n_gt_1_rejected() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![msg("user", "x")],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: Some(2),
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::MultipleChoices)
        ));
    }

    #[test]
    fn tool_role_becomes_tool_result() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![ChatMessage {
                role: "tool".into(),
                content: Some(MessageContent::Text("42".into())),
                tool_calls: vec![],
                tool_call_id: Some("call_1".into()),
                name: None,
            }],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
        assert_eq!(v2.messages.len(), 1);
        match &v2.messages[0].content[0] {
            ContentBlock::ToolResult { tool_call_id, .. } => {
                assert_eq!(tool_call_id.as_str(), "call_1")
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn assistant_tool_call_maps_to_tooluse() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![ChatMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: vec![ToolCallReplay {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: ToolCallFunction {
                        name: "get_weather".into(),
                        arguments: r#"{"city":"Paris"}"#.into(),
                    },
                }],
                tool_call_id: None,
                name: None,
            }],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![ToolDecl {
                kind: "function".into(),
                function: ToolDeclFunction {
                    name: "get_weather".into(),
                    description: "gets weather".into(),
                    parameters: serde_json::json!({"type":"object"}),
                },
            }],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
        assert_eq!(v2.tools.len(), 1);
        match &v2.messages[0].content[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "Paris");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn bad_tool_args_rejected() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![ChatMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: vec![ToolCallReplay {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: ToolCallFunction {
                        name: "f".into(),
                        arguments: "not json".into(),
                    },
                }],
                tool_call_id: None,
                name: None,
            }],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::BadToolArgs(_))
        ));
    }

    #[test]
    fn text_frames_stream_as_content() {
        let mut b = ChunkBuilder::new("id".into(), "m".into(), 0);
        let f = ResponseV2::Frame {
            id: "id".into(),
            block: ResponseBlock::Text {
                delta: "Par".into(),
            },
        };
        let chunk = b.ingest(&f).unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Par"));
        assert_eq!(chunk.choices[0].delta.role.as_deref(), Some("assistant"));
        // second text frame: no role repeat
        let f2 = ResponseV2::Frame {
            id: "id".into(),
            block: ResponseBlock::Text { delta: "is".into() },
        };
        let c2 = b.ingest(&f2).unwrap();
        assert_eq!(c2.choices[0].delta.role, None);
    }

    #[test]
    fn thinking_frames_are_dropped() {
        let mut b = ChunkBuilder::new("id".into(), "m".into(), 0);
        let f = ResponseV2::Frame {
            id: "id".into(),
            block: ResponseBlock::Thinking {
                delta: "hmm".into(),
            },
        };
        assert!(b.ingest(&f).is_none());
    }

    #[test]
    fn finalize_emits_usage_and_finish_reason() {
        let b = ChunkBuilder::new("id".into(), "m".into(), 0);
        let chunk = b.finalize(
            &UsageV2 {
                input_tokens: 10,
                output_tokens: 5,
            },
            StopReasonV2::EndTurn,
        );
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(chunk.usage.as_ref().unwrap().total_tokens, 15);
    }

    #[test]
    fn embeddings_input_string_and_array() {
        let (r1, fmt1) = embeddings_request_to_inferd(
            EmbeddingsRequest {
                model: "m".into(),
                input: serde_json::json!("hello"),
                dimensions: Some(256),
                encoding_format: None,
            },
            "i".into(),
        )
        .unwrap();
        assert_eq!(r1.input, vec!["hello".to_string()]);
        assert_eq!(r1.dimensions, Some(256));
        assert_eq!(fmt1, EncodingFormat::Float); // omitted → float

        let (r2, _) = embeddings_request_to_inferd(
            EmbeddingsRequest {
                model: "m".into(),
                input: serde_json::json!(["a", "b"]),
                dimensions: None,
                encoding_format: None,
            },
            "i".into(),
        )
        .unwrap();
        assert_eq!(r2.input.len(), 2);
    }

    #[test]
    fn base64_encoding_requested_and_encoded() {
        let (_, fmt) = embeddings_request_to_inferd(
            EmbeddingsRequest {
                model: "m".into(),
                input: serde_json::json!("x"),
                dimensions: None,
                encoding_format: Some("base64".into()),
            },
            "i".into(),
        )
        .unwrap();
        assert_eq!(fmt, EncodingFormat::Base64);

        // One f32 (1.0 = 0x3F800000 LE bytes 00 00 80 3F) → base64 "AACAPw==".
        let resp =
            embeddings_response_to_openai(vec![vec![1.0]], "m".into(), 1, EncodingFormat::Base64);
        match &resp.data[0].embedding {
            EmbeddingVector::Base64(s) => assert_eq!(s, "AACAPw=="),
            other => panic!("expected base64, got {other:?}"),
        }
    }

    #[test]
    fn bad_encoding_format_rejected() {
        let e = embeddings_request_to_inferd(
            EmbeddingsRequest {
                model: "m".into(),
                input: serde_json::json!("x"),
                dimensions: None,
                encoding_format: Some("protobuf".into()),
            },
            "i".into(),
        );
        assert!(matches!(e, Err(TranslateError::BadEncodingFormat(_))));
    }

    // A 1x1 red PNG, base64 (same fixture as image_decode tests).
    const RED_1X1_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn parts_msg(role: &str, parts: Vec<ContentPart>) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: Some(MessageContent::Parts(parts)),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }
    }

    fn image_part(b64: &str) -> ContentPart {
        ContentPart::ImageUrl {
            image_url: inferd_openai_wire::ImageUrl {
                url: format!("data:image/png;base64,{b64}"),
                detail: None,
            },
        }
    }

    #[test]
    fn user_message_with_text_and_image_maps_to_blocks_and_attachment() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg(
                "user",
                vec![
                    ContentPart::Text {
                        text: "what is this?".into(),
                    },
                    image_part(RED_1X1_PNG_B64),
                ],
            )],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "id".into(), None).unwrap();
        // One message, two content blocks (text + image ref).
        assert_eq!(v2.messages.len(), 1);
        assert_eq!(v2.messages[0].content.len(), 2);
        assert!(matches!(
            &v2.messages[0].content[0],
            ContentBlock::Text { text } if text == "what is this?"
        ));
        let att_id = match &v2.messages[0].content[1] {
            ContentBlock::Image { attachment_id } => attachment_id.clone(),
            other => panic!("expected Image block, got {other:?}"),
        };
        // One attachment, id-correlated, carrying raw RGB (1*1*3 = 3 bytes).
        assert_eq!(v2.attachments.len(), 1);
        match &v2.attachments[0] {
            Attachment::Image {
                id, width, height, ..
            } => {
                assert_eq!(id, &att_id);
                assert_eq!(*width, 1);
                assert_eq!(*height, 1);
            }
            other => panic!("expected Image attachment, got {other:?}"),
        }
        assert_eq!(v2.attachments[0].bytes().len(), 3);
    }

    #[test]
    fn two_images_get_distinct_ids() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg(
                "user",
                vec![image_part(RED_1X1_PNG_B64), image_part(RED_1X1_PNG_B64)],
            )],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "id".into(), None).unwrap();
        assert_eq!(v2.attachments.len(), 2);
        assert_ne!(v2.attachments[0].id(), v2.attachments[1].id());
        // The request must resolve (every image block's id maps to an
        // attachment) — this is the invariant the daemon enforces.
        assert!(v2.resolve().is_ok());
    }

    #[test]
    fn image_on_system_message_rejected() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg("system", vec![image_part(RED_1X1_PNG_B64)])],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::ImageOnNonUser(_))
        ));
    }

    #[test]
    fn remote_image_url_rejected() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg(
                "user",
                vec![ContentPart::ImageUrl {
                    image_url: inferd_openai_wire::ImageUrl {
                        url: "https://evil.example/x.png".into(),
                        detail: None,
                    },
                }],
            )],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::Image(_))
        ));
    }

    #[test]
    fn response_format_json_schema_maps_to_grammar() {
        let schema = serde_json::json!({"type":"object","properties":{"x":{"type":"number"}}});
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![msg("user", "give x")],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: Some(inferd_openai_wire::ResponseFormat::JsonSchema {
                json_schema: inferd_openai_wire::JsonSchemaSpec {
                    name: Some("s".into()),
                    schema: Some(schema.clone()),
                    strict: Some(true),
                },
            }),
        };
        let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
        match v2.response_format {
            Some(ResponseFormat::JsonSchema { schema: s }) => assert_eq!(s, schema),
            other => panic!("expected JsonSchema grammar, got {other:?}"),
        }
    }

    #[test]
    fn response_format_text_and_json_object_are_unconstrained() {
        for rf in [
            inferd_openai_wire::ResponseFormat::Text,
            inferd_openai_wire::ResponseFormat::JsonObject,
        ] {
            let req = ChatRequest {
                model: "m".into(),
                messages: vec![msg("user", "hi")],
                stream: false,
                temperature: None,
                top_p: None,
                max_tokens: None,
                n: None,
                tools: vec![],
                tool_choice: None,
                stream_options: None,
                response_format: Some(rf),
            };
            let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
            assert!(
                v2.response_format.is_none(),
                "text/json_object carry no schema → unconstrained"
            );
        }
    }

    #[test]
    fn too_many_images_rejected() {
        // One more than the cap → refused before the (n+1)th decode.
        let parts: Vec<ContentPart> = (0..=MAX_IMAGES_PER_REQUEST)
            .map(|_| image_part(RED_1X1_PNG_B64))
            .collect();
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg("user", parts)],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::TooManyImages(_))
        ));
    }

    #[test]
    fn images_at_the_cap_are_allowed() {
        // Exactly the cap succeeds (1x1 images are tiny; no budget issue).
        let parts: Vec<ContentPart> = (0..MAX_IMAGES_PER_REQUEST)
            .map(|_| image_part(RED_1X1_PNG_B64))
            .collect();
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg("user", parts)],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
        assert_eq!(v2.attachments.len(), MAX_IMAGES_PER_REQUEST);
    }

    // --- audio ---------------------------------------------------------

    /// A minimal 16 kHz mono RIFF/WAVE clip of `frames` samples, base64'd.
    /// Hand-rolled so no binary fixture ships (same approach as
    /// `audio_decode`'s tests).
    fn wav16k_b64(frames: usize) -> String {
        let data_len = (frames * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&16_000u32.to_le_bytes());
        w.extend_from_slice(&32_000u32.to_le_bytes()); // byte rate
        w.extend_from_slice(&2u16.to_le_bytes()); // block align
        w.extend_from_slice(&16u16.to_le_bytes()); // bits
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for n in 0..frames {
            w.extend_from_slice(&(((n % 100) as i16) * 300 - 15_000).to_le_bytes());
        }
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&w)
    }

    fn audio_part(frames: usize) -> ContentPart {
        ContentPart::InputAudio {
            input_audio: inferd_openai_wire::InputAudio {
                data: wav16k_b64(frames),
                format: "wav".into(),
            },
        }
    }

    fn audio_req(role: &str, parts: Vec<ContentPart>) -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg(role, parts)],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        }
    }

    #[test]
    fn user_message_with_audio_maps_to_block_and_attachment() {
        let req = audio_req(
            "user",
            vec![
                ContentPart::Text {
                    text: "transcribe this".into(),
                },
                audio_part(1600),
            ],
        );
        let v2 = chat_request_to_v2(req, "id".into(), Some(16_000)).unwrap();
        assert_eq!(v2.messages[0].content.len(), 2);
        let att_id = match &v2.messages[0].content[1] {
            ContentBlock::Audio { attachment_id } => attachment_id.clone(),
            other => panic!("expected Audio block, got {other:?}"),
        };
        assert_eq!(v2.attachments.len(), 1);
        match &v2.attachments[0] {
            Attachment::Audio {
                id, sample_rate, ..
            } => {
                assert_eq!(id, &att_id);
                // The advertised rate, carried through unchanged — the
                // daemon rejects anything else.
                assert_eq!(*sample_rate, 16_000);
            }
            other => panic!("expected Audio attachment, got {other:?}"),
        }
        // Mono LE f32: 4 octets per sample, no resampling at the target rate.
        assert_eq!(v2.attachments[0].bytes().len(), 1600 * 4);
        assert!(v2.resolve().is_ok());
    }

    #[test]
    fn audio_and_image_ids_do_not_collide() {
        let req = audio_req("user", vec![image_part(RED_1X1_PNG_B64), audio_part(320)]);
        let v2 = chat_request_to_v2(req, "id".into(), Some(16_000)).unwrap();
        assert_eq!(v2.attachments.len(), 2);
        // Per-modality counters, so the image keeps `img-0` even though the
        // audio shares the attachment vec.
        assert_eq!(v2.attachments[0].id(), "img-0");
        assert_eq!(v2.attachments[1].id(), "aud-0");
        assert!(v2.resolve().is_ok());
    }

    #[test]
    fn audio_without_an_advertised_rate_rejected() {
        // `None` means no backend advertised an audio rate. Guessing 16000
        // would produce a confidently-wrong answer, so refuse instead.
        let req = audio_req("user", vec![audio_part(320)]);
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::AudioUnsupported)
        ));
    }

    #[test]
    fn audio_on_system_message_rejected() {
        let req = audio_req("system", vec![audio_part(320)]);
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), Some(16_000)),
            Err(TranslateError::AudioOnNonUser(_))
        ));
    }

    #[test]
    fn too_many_audio_clips_rejected() {
        let parts: Vec<ContentPart> = (0..=MAX_AUDIO_CLIPS_PER_REQUEST)
            .map(|_| audio_part(160))
            .collect();
        let req = audio_req("user", parts);
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), Some(16_000)),
            Err(TranslateError::TooManyAudioClips(_))
        ));
    }

    #[test]
    fn audio_clips_at_the_cap_are_allowed() {
        let parts: Vec<ContentPart> = (0..MAX_AUDIO_CLIPS_PER_REQUEST)
            .map(|_| audio_part(160))
            .collect();
        let req = audio_req("user", parts);
        let v2 = chat_request_to_v2(req, "i".into(), Some(16_000)).unwrap();
        assert_eq!(v2.attachments.len(), MAX_AUDIO_CLIPS_PER_REQUEST);
        assert!(v2.resolve().is_ok());
    }

    #[test]
    fn audio_resampled_to_the_advertised_rate() {
        // The clip is 16 kHz; the backend wants 8 kHz. The bridge converts
        // rather than passing the source rate through.
        let req = audio_req("user", vec![audio_part(16_000)]);
        let v2 = chat_request_to_v2(req, "i".into(), Some(8_000)).unwrap();
        match &v2.attachments[0] {
            Attachment::Audio { sample_rate, .. } => assert_eq!(*sample_rate, 8_000),
            other => panic!("expected Audio attachment, got {other:?}"),
        }
        let samples = v2.attachments[0].bytes().len() / 4;
        assert!(
            (samples as f64 - 8_000.0).abs() / 8_000.0 < 0.01,
            "expected ~8000 samples, got {samples}"
        );
    }

    #[test]
    fn undecodable_audio_rejected() {
        let req = audio_req(
            "user",
            vec![ContentPart::InputAudio {
                input_audio: inferd_openai_wire::InputAudio {
                    data: "aGVsbG8=".into(), // "hello", not audio
                    format: "wav".into(),
                },
            }],
        );
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), Some(16_000)),
            Err(TranslateError::Audio(_))
        ));
    }

    #[test]
    fn text_only_parts_array_still_works() {
        // A parts array with only text (no image) — common when a client
        // always uses the array form. Should map identically to a string.
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![parts_msg(
                "user",
                vec![ContentPart::Text {
                    text: "hello".into(),
                }],
            )],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
        assert_eq!(v2.messages.len(), 1);
        assert_eq!(v2.messages[0].content.len(), 1);
        assert!(v2.attachments.is_empty());
    }

    #[test]
    fn embeddings_response_float_shape() {
        let resp = embeddings_response_to_openai(
            vec![vec![0.1, 0.2], vec![0.3, 0.4]],
            "m".into(),
            7,
            EncodingFormat::Float,
        );
        assert_eq!(resp.object, "list");
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[1].index, 1);
        assert_eq!(resp.usage.total_tokens, 7);
        match &resp.data[0].embedding {
            EmbeddingVector::Floats(v) => assert_eq!(v.len(), 2),
            other => panic!("expected floats, got {other:?}"),
        }
    }

    /// A `tool_choice` string maps onto the daemon's constraint, and the
    /// `n`-of-`tools` guard fires before it: the daemon rejects a
    /// `tool_choice` with no tools, so catching it here gives a 400 with
    /// a bridge-shaped message instead of an IPC round trip.
    #[test]
    fn tool_choice_modes_map_when_tools_are_declared() {
        for (mode, want) in [
            (ToolChoiceMode::Auto, ToolChoice::Auto),
            (ToolChoiceMode::Required, ToolChoice::Required),
            (ToolChoiceMode::None, ToolChoice::None),
        ] {
            let mut req = req_with_one_tool();
            req.tool_choice = Some(OpenAiToolChoice::Mode(mode));
            let v2 = chat_request_to_v2(req, "i".into(), None).unwrap();
            assert_eq!(v2.tool_choice, Some(want), "{mode:?}");
        }
    }

    #[test]
    fn tool_choice_without_tools_is_rejected() {
        let mut req = req_with_one_tool();
        req.tools = vec![];
        req.tool_choice = Some(OpenAiToolChoice::Mode(ToolChoiceMode::Required));
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::ToolChoiceWithoutTools)
        ));
    }

    /// Naming a function is rejected rather than widened to `required`:
    /// widening would let the model call a *different* declared tool
    /// while the caller believed it had pinned one.
    #[test]
    fn named_tool_choice_is_rejected_not_widened() {
        let mut req = req_with_one_tool();
        req.tool_choice = Some(OpenAiToolChoice::Named(NamedToolChoice {
            kind: "function".into(),
            function: NamedToolChoiceFunction {
                name: "get_weather".into(),
            },
        }));
        match chat_request_to_v2(req, "i".into(), None) {
            Err(TranslateError::NamedToolChoice(name)) => assert_eq!(name, "get_weather"),
            other => panic!("expected NamedToolChoice, got {other:?}"),
        }
    }

    /// Unlike `response_format`, an unrecognised `tool_choice` is an
    /// error, not a silent drop: the caller asked for a guarantee.
    #[test]
    fn unrecognised_tool_choice_is_rejected() {
        let mut req = req_with_one_tool();
        req.tool_choice = Some(OpenAiToolChoice::Mode(ToolChoiceMode::Other));
        assert!(matches!(
            chat_request_to_v2(req, "i".into(), None),
            Err(TranslateError::BadToolChoice)
        ));
    }

    /// Absent `tool_choice` must stay absent: a request that declares
    /// tools without a choice keeps the daemon's default behaviour.
    #[test]
    fn absent_tool_choice_stays_absent() {
        let v2 = chat_request_to_v2(req_with_one_tool(), "i".into(), None).unwrap();
        assert_eq!(v2.tool_choice, None);
    }

    fn req_with_one_tool() -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            messages: vec![msg("user", "weather in Paris?")],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            n: None,
            tools: vec![ToolDecl {
                kind: "function".into(),
                function: ToolDeclFunction {
                    name: "get_weather".into(),
                    description: "gets weather".into(),
                    parameters: serde_json::json!({"type":"object"}),
                },
            }],
            tool_choice: None,
            stream_options: None,
            response_format: None,
        }
    }
}
