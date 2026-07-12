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
    ResponseFormat as OpenAiResponseFormat,
};
use inferd_proto::embed::EmbedRequest;
use inferd_proto::v2::{
    Attachment, ContentBlock, MessageV2, RequestV2, ResponseBlock, ResponseFormat, ResponseV2,
    RoleV2, StopReasonV2, Tool, ToolCallId, UsageV2,
};
use thiserror::Error;

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
    /// The decoded images exceeded the aggregate byte budget. Bounds the
    /// bridge's peak RGB memory: many small-compressed / large-decoded
    /// images (each individually legal) can otherwise sum to gigabytes.
    #[error("decoded image bytes exceed the {0}-byte per-request budget")]
    ImageBudgetExceeded(usize),
}

/// Max number of image parts accepted in one chat request. A single
/// visual question rarely needs more than a handful of frames; a large
/// count is almost always abuse, and each image is a full RGB buffer the
/// bridge holds until it forwards the request.
pub const MAX_IMAGES_PER_REQUEST: usize = 8;

/// Aggregate decoded-RGB budget across all images in one request. Caps
/// the bridge's peak memory regardless of how the (compressed) images
/// packed into the 8 MiB HTTP body — a decompression-amplification guard
/// the per-image limit alone does not provide. 128 MiB comfortably fits
/// several full-resolution images while refusing a bomb.
pub const MAX_TOTAL_DECODED_IMAGE_BYTES: usize = 128 * 1024 * 1024;

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

/// Translate an inbound OpenAI chat request into an inferd `RequestV2`.
pub fn chat_request_to_v2(req: ChatRequest, id: String) -> Result<RequestV2, TranslateError> {
    if req.n.unwrap_or(1) > 1 {
        return Err(TranslateError::MultipleChoices);
    }
    let response_format = map_response_format(req.response_format);

    let mut messages = Vec::with_capacity(req.messages.len());
    // Image bytes ride out-of-band as attachments referenced by id; the
    // daemon reassembles them into BLOB frames. Ids are unique across the
    // whole request (`img-<n>`), assigned as images are encountered.
    let mut attachments: Vec<Attachment> = Vec::new();
    // Running total of decoded RGB bytes across all images in the
    // request, checked against the aggregate budget after each decode.
    let mut total_image_bytes: usize = 0;
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
                            if attachments.len() >= MAX_IMAGES_PER_REQUEST {
                                return Err(TranslateError::TooManyImages(MAX_IMAGES_PER_REQUEST));
                            }
                            let decoded = image_decode::decode_image_url(&image_url.url)?;
                            // Aggregate-byte guard: many individually-legal
                            // images can still sum to gigabytes of retained
                            // RGB (a compression-amplification DoS). Cap the
                            // running total.
                            total_image_bytes = total_image_bytes.saturating_add(decoded.rgb.len());
                            if total_image_bytes > MAX_TOTAL_DECODED_IMAGE_BYTES {
                                return Err(TranslateError::ImageBudgetExceeded(
                                    MAX_TOTAL_DECODED_IMAGE_BYTES,
                                ));
                            }
                            let id = format!("img-{}", attachments.len());
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

    Ok(RequestV2 {
        id,
        messages,
        attachments,
        tools,
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
        ChatMessage, ToolCallFunction, ToolCallReplay, ToolDecl, ToolDeclFunction,
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "id1".into()).unwrap();
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
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into()),
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into()).unwrap();
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into()).unwrap();
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
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into()),
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "id".into()).unwrap();
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "id".into()).unwrap();
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
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into()),
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
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into()),
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
            stream_options: None,
            response_format: Some(inferd_openai_wire::ResponseFormat::JsonSchema {
                json_schema: inferd_openai_wire::JsonSchemaSpec {
                    name: Some("s".into()),
                    schema: Some(schema.clone()),
                    strict: Some(true),
                },
            }),
        };
        let v2 = chat_request_to_v2(req, "i".into()).unwrap();
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
                stream_options: None,
                response_format: Some(rf),
            };
            let v2 = chat_request_to_v2(req, "i".into()).unwrap();
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
            stream_options: None,
            response_format: None,
        };
        assert!(matches!(
            chat_request_to_v2(req, "i".into()),
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into()).unwrap();
        assert_eq!(v2.attachments.len(), MAX_IMAGES_PER_REQUEST);
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
            stream_options: None,
            response_format: None,
        };
        let v2 = chat_request_to_v2(req, "i".into()).unwrap();
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
}
