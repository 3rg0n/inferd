package inferd

// v2 wire protocol — typed content blocks, attachments, and tools.
//
// The v2 surface is documented in ADR 0015 (typed content blocks) as
// amended by ADR 0016 (consumer decodes media before sending) and
// ADR 0013 (the daemon is the gateway that shapes semantic intent into
// engine input). As of v0.4 (ADR 0021) it is the single generation
// surface — frozen on the neutral socket (`inferd.sock` on Unix,
// `\\.\pipe\inferd` on Windows) — using length-prefixed, type-tagged
// framing with an in-band wire_version. These types are byte-compatible
// with the Rust `inferd-proto` v2 module (request.rs / attachment.rs /
// response.rs / tool.rs).
//
// Media decode posture (ADR 0016 + ADR 0021): attachment `Bytes`
// carries *already-decoded* raw payloads — interleaved RGB for images
// (width*height*3 octets, no alpha), little-endian float32 PCM for
// audio — and travels out-of-band as a raw BLOB frame keyed by
// attachment id (no base64). The daemon links no image/audio codec;
// the consumer decodes before sending.

import "encoding/json"

// ContentType discriminates ContentBlock variants on the wire (the
// JSON `type` tag).
type ContentType string

const (
	ContentText       ContentType = "text"
	ContentImage      ContentType = "image"
	ContentAudio      ContentType = "audio"
	ContentVideo      ContentType = "video"
	ContentToolUse    ContentType = "tool_use"
	ContentToolResult ContentType = "tool_result"
)

// ContentBlock is one element of MessageV2.Content. The set of
// populated fields depends on Type:
//
//   - text:        Text
//   - image/audio/video: AttachmentID (references AttachmentV2.ID)
//   - tool_use:    ToolCallID, Name, Input (assistant turns being replayed)
//   - tool_result: ToolCallID, Content (consumer-supplied tool output)
//
// Helper constructors (TextBlock, ImageBlock, …) cover the common
// cases. Unknown `type` values from a newer daemon decode with Type
// set and other fields zero; callers should ignore them rather than
// error (forward-compat, matching the Rust `Unknown` variant).
type ContentBlock struct {
	Type ContentType `json:"type"`
	// text
	Text string `json:"text,omitempty"`
	// image / audio / video
	AttachmentID string `json:"attachment_id,omitempty"`
	// tool_use / tool_result
	ToolCallID string `json:"tool_call_id,omitempty"`
	// tool_use
	Name  string          `json:"name,omitempty"`
	Input json.RawMessage `json:"input,omitempty"`
	// tool_result
	Content []ContentBlock `json:"content,omitempty"`
}

// TextBlock builds a text content block.
func TextBlock(text string) ContentBlock {
	return ContentBlock{Type: ContentText, Text: text}
}

// ImageBlock builds an image content block referencing an attachment
// id present in the request's Attachments table.
func ImageBlock(attachmentID string) ContentBlock {
	return ContentBlock{Type: ContentImage, AttachmentID: attachmentID}
}

// AudioBlock builds an audio content block referencing an attachment id.
func AudioBlock(attachmentID string) ContentBlock {
	return ContentBlock{Type: ContentAudio, AttachmentID: attachmentID}
}

// MessageV2 is one turn in the v2 conversation history. Content must be
// non-empty.
type MessageV2 struct {
	Role    Role           `json:"role"`
	Content []ContentBlock `json:"content"`
}

// AttachmentKind discriminates AttachmentV2 variants (the JSON `kind`
// tag).
type AttachmentKind string

const (
	AttachmentImage AttachmentKind = "image"
	AttachmentAudio AttachmentKind = "audio"
	AttachmentVideo AttachmentKind = "video"
)

// AttachmentV2 is one binary payload in the request's top-level
// Attachments table, referenced by ID from image/audio/video content
// blocks.
//
// As of v0.4 (ADR 0021) the raw bytes do NOT travel inside the request
// JSON — Bytes is `json:"-"` and rides out-of-band in a length-prefixed
// BLOB frame, preceded by a BlobDescriptor naming this ID. Bytes are
// the *raw* decoded payload (interleaved RGB for images, little-endian
// f32 PCM for audio); no base64. The JSON carries only metadata
// (kind/id/width/height/sample_rate). Decode posture is unchanged
// (ADR 0016): the consumer still decodes media to raw bytes.
type AttachmentV2 struct {
	Kind AttachmentKind `json:"kind"`
	ID   string         `json:"id"`
	// image
	Width  uint32 `json:"width,omitempty"`
	Height uint32 `json:"height,omitempty"`
	// audio
	SampleRate uint32 `json:"sample_rate,omitempty"`
	// Raw decoded bytes, sent in a BLOB frame (not this JSON object).
	Bytes []byte `json:"-"`
}

// ImageAttachment builds an image attachment from raw interleaved RGB
// bytes (width*height*3 octets). The caller decodes the source image
// (JPEG/PNG/…) to RGB before calling this — the daemon links no codec
// (ADR 0016). The bytes are sent verbatim in a BLOB frame.
func ImageAttachment(id string, width, height uint32, rgb []byte) AttachmentV2 {
	return AttachmentV2{
		Kind:   AttachmentImage,
		ID:     id,
		Width:  width,
		Height: height,
		Bytes:  rgb,
	}
}

// AudioAttachment builds an audio attachment from raw little-endian
// float32 PCM samples (mono, 4 octets per sample). The caller decodes the
// source audio (WAV/MP3/…) to f32 PCM before calling this — the daemon
// links no codec (ADR 0016).
//
// sampleRate MUST equal the rate the backend advertises as
// `audio_sample_rate` on its capabilities frame (see
// AdminEvent.RequiredAudioSampleRate). The daemon rejects any other rate
// with an invalid_request error naming both rates; it does not resample.
// Sending 44.1 kHz audio to a 16 kHz encoder would otherwise time-scale it
// ~2.75x and produce a confidently wrong answer.
func AudioAttachment(id string, sampleRate uint32, pcmF32LE []byte) AttachmentV2 {
	return AttachmentV2{
		Kind:       AttachmentAudio,
		ID:         id,
		SampleRate: sampleRate,
		Bytes:      pcmF32LE,
	}
}

// BlobDescriptor is the JSON control frame that precedes each
// attachment BLOB frame (ADR 0021), correlating the raw bytes to an
// attachment by ID.
type BlobDescriptor struct {
	// Type is always "attachment_blob".
	Type string `json:"type"`
	// AttachmentID is the AttachmentV2.ID the following BLOB belongs to.
	AttachmentID string `json:"attachment_id"`
	// Len is the byte length of the following BLOB frame.
	Len uint64 `json:"len"`
}

// ToolV2 is a tool definition the model may call. InputSchema is a JSON
// Schema object describing the tool's arguments.
type ToolV2 struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	InputSchema json.RawMessage `json:"input_schema"`
}

// ToolChoice constrains whether the model may, must, or must not call a
// tool. It is a constraint rather than a hint on backends that advertise
// tool support: the llamacpp backend compiles the loaded family's
// tool-call syntax to a grammar and installs it on the sampler, so
// ToolChoiceRequired cannot come back as prose.
//
// The daemon rejects a ToolChoice sent without Tools, and rejects it
// alongside ResponseFormat — only one grammar can be installed, so
// honouring either would silently drop the other.
type ToolChoice string

const (
	// ToolChoiceAuto lets the model decide. Equivalent to omitting the
	// field, except the daemon additionally constrains the *shape* of a
	// call the model chooses to make.
	ToolChoiceAuto ToolChoice = "auto"
	// ToolChoiceRequired forces at least one tool call: no path through
	// sampling produces a bare text answer.
	ToolChoiceRequired ToolChoice = "required"
	// ToolChoiceNone forbids a tool call. Tool declarations still reach
	// the prompt, so the rendered context is unchanged.
	ToolChoiceNone ToolChoice = "none"
)

// RequestV2 is the v2 inference request envelope (ADR 0015). Populate
// ID and Messages; Attachments / Tools / sampling fields are optional.
//
// Pointer-typed sampling fields distinguish "omitted" (daemon applies
// the backend's default) from "explicitly zero", matching the v1
// Request convention.
// ResponseFormat specifies a structured output constraint for generation.
//
// The Type field discriminates variants (e.g. "json_schema"). The Schema
// field (when populated) contains a JSON Schema the model output must conform
// to. The daemon translates the schema to engine-specific constraints
// (e.g. GBNF grammar for llamacpp).
type ResponseFormat struct {
	Type   string          `json:"type"`                    // "json_schema"
	Schema json.RawMessage `json:"schema,omitempty"`        // JSON Schema bytes
}

// JSONSchemaFormat is a helper to construct a ResponseFormat for JSON Schema
// structured output. The schema argument should be marshalled JSON bytes.
func JSONSchemaFormat(schema json.RawMessage) *ResponseFormat {
	return &ResponseFormat{
		Type:   "json_schema",
		Schema: schema,
	}
}

type RequestV2 struct {
	// WireVersion is set automatically by Client.GenerateV2 to
	// WireVersion (ADR 0021); a daemon rejects a mismatch loudly.
	WireVersion  uint32           `json:"wire_version"`
	ID           string           `json:"id,omitempty"`
	Messages     []MessageV2      `json:"messages"`
	Attachments  []AttachmentV2   `json:"attachments,omitempty"`
	Tools        []ToolV2         `json:"tools,omitempty"`
	// ToolChoice constrains tool use. Empty means omitted (the daemon
	// applies its default); see ToolChoice for the values. Requires a
	// non-empty Tools.
	ToolChoice   ToolChoice       `json:"tool_choice,omitempty"`
	Temperature  *float64         `json:"temperature,omitempty"`
	TopP         *float64         `json:"top_p,omitempty"`
	TopK         *uint32          `json:"top_k,omitempty"`
	MaxTokens    *uint32          `json:"max_tokens,omitempty"`
	Stream       *bool            `json:"stream,omitempty"`
	ResponseFormat *ResponseFormat `json:"response_format,omitempty"`
	// Thinking requests reasoning mode: when *true, the daemon asks the
	// model to produce an internal reasoning trace, separated onto
	// `thinking` response blocks (not leaked into user-visible text).
	// nil/false = no thinking (default). Backends without reasoning
	// support ignore it. (Gemma 4: daemon injects <|think|> into the
	// system turn.)
	Thinking *bool `json:"thinking,omitempty"`
}

// ResponseV2Type discriminates v2 response frames on the wire.
type ResponseV2Type string

const (
	ResponseV2Frame ResponseV2Type = "frame"
	ResponseV2Done  ResponseV2Type = "done"
	ResponseV2Error ResponseV2Type = "error"
)

// BlockType discriminates the streaming-output block inside a v2
// `frame` response.
type BlockType string

const (
	BlockText     BlockType = "text"
	BlockThinking BlockType = "thinking"
	BlockToolUse  BlockType = "tool_use"
)

// ResponseBlockV2 is the payload of a v2 `frame` response: an
// incremental text delta, a reasoning-trace delta, or a complete
// tool-use request.
type ResponseBlockV2 struct {
	Type BlockType `json:"type"`
	// text / thinking
	Delta string `json:"delta,omitempty"`
	// tool_use
	ToolCallID string          `json:"tool_call_id,omitempty"`
	Name       string          `json:"name,omitempty"`
	Input      json.RawMessage `json:"input,omitempty"`
}

// StopReasonV2 on a v2 done frame.
type StopReasonV2 string

const (
	StopEndTurn      StopReasonV2 = "end_turn"
	StopMaxTokens    StopReasonV2 = "max_tokens"
	StopToolUse      StopReasonV2 = "tool_use"
	StopStopSequence StopReasonV2 = "stop_sequence"
	StopV2Cancelled  StopReasonV2 = "cancelled"
	StopV2Error      StopReasonV2 = "error"
)

// ErrorCodeV2 classifies v2 error frames (ADR 0015, extended by
// ADR 0021). Superset of the v1 ErrorCode taxonomy. Must stay in sync
// with the Rust `ErrorCodeV2` enum in inferd-proto's v2/response.rs.
type ErrorCodeV2 string

const (
	ErrV2QueueFull             ErrorCodeV2 = "queue_full"
	ErrV2BackendUnavailable    ErrorCodeV2 = "backend_unavailable"
	ErrV2InvalidRequest        ErrorCodeV2 = "invalid_request"
	ErrV2FrameTooLarge         ErrorCodeV2 = "frame_too_large"
	ErrV2Internal              ErrorCodeV2 = "internal"
	ErrV2AttachmentUnsupported ErrorCodeV2 = "attachment_unsupported"
	ErrV2ToolCallMalformed     ErrorCodeV2 = "tool_call_malformed"
	// ErrV2WireVersionUnsupported is returned when the daemon does not
	// speak the request's wire_version (ADR 0021). The terminal error
	// frame's Message names both the requested and supported versions.
	ErrV2WireVersionUnsupported ErrorCodeV2 = "wire_version_unsupported"
)

// UsageV2 is the token-count report on a v2 done frame. Field names
// match Anthropic's shape (input/output) rather than v1's prompt/
// completion; the underlying counts are the same.
type UsageV2 struct {
	InputTokens  uint32 `json:"input_tokens"`
	OutputTokens uint32 `json:"output_tokens"`
}

// ResponseV2 is one frame off the v2 response stream. Variant is
// selected by Type:
//
//   - frame: Block          (streaming text / thinking / tool_use)
//   - done:  Usage, StopReason, Backend
//   - error: Code, Message
type ResponseV2 struct {
	ID         string           `json:"id"`
	Type       ResponseV2Type   `json:"type"`
	Block      *ResponseBlockV2 `json:"block,omitempty"`       // frame
	Usage      *UsageV2         `json:"usage,omitempty"`       // done
	StopReason StopReasonV2     `json:"stop_reason,omitempty"` // done
	Backend    string           `json:"backend,omitempty"`     // done — diagnostic only
	Code       ErrorCodeV2      `json:"code,omitempty"`        // error
	Message    string           `json:"message,omitempty"`     // error
}

// IsTerminal reports whether this frame ends a v2 request stream.
func (r ResponseV2) IsTerminal() bool {
	return r.Type == ResponseV2Done || r.Type == ResponseV2Error
}
