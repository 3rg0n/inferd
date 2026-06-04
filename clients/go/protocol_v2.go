package inferd

// v2 wire protocol — typed content blocks, attachments, and tools.
//
// The v2 surface is documented in ADR 0015 (typed content blocks) as
// amended by ADR 0016 (consumer decodes media before sending) and
// ADR 0013 (the daemon is the gateway that shapes semantic intent into
// engine input). It is frozen on its own socket — `infer.v2.sock` on
// Unix, `\\.\pipe\inferd-infer-v2` on Windows — independently of the v1
// generation socket. These types are byte-compatible with the Rust
// `inferd-proto` v2 module (request.rs / attachment.rs / response.rs /
// tool.rs).
//
// Media decode posture (ADR 0016): attachment `Bytes` carries
// *already-decoded* payloads, base64-encoded — raw interleaved RGB for
// images (width*height*3 octets, no alpha), little-endian float32 PCM
// for audio. The daemon links no image/audio codec; the consumer
// decodes before sending.

import (
	"encoding/base64"
	"encoding/json"
)

// base64Encode is the standard RFC-4648 base64 (with padding) the v2
// wire uses for attachment bytes, matching the Rust side's
// base64::engine::general_purpose::STANDARD.
func base64Encode(b []byte) string {
	return base64.StdEncoding.EncodeToString(b)
}

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
// blocks. Bytes is base64 of the already-decoded payload (ADR 0016).
//
// The populated metadata depends on Kind: image carries Width+Height,
// audio carries SampleRate, video carries neither (reserved).
type AttachmentV2 struct {
	Kind AttachmentKind `json:"kind"`
	ID   string         `json:"id"`
	// image
	Width  uint32 `json:"width,omitempty"`
	Height uint32 `json:"height,omitempty"`
	// audio
	SampleRate uint32 `json:"sample_rate,omitempty"`
	// base64 of the decoded bytes (all kinds)
	Bytes string `json:"bytes"`
}

// ImageAttachment builds an image attachment from raw interleaved RGB
// bytes (width*height*3 octets). The caller decodes the source image
// (JPEG/PNG/…) to RGB before calling this — the daemon links no codec
// (ADR 0016). Bytes are base64-encoded here.
func ImageAttachment(id string, width, height uint32, rgb []byte) AttachmentV2 {
	return AttachmentV2{
		Kind:   AttachmentImage,
		ID:     id,
		Width:  width,
		Height: height,
		Bytes:  base64Encode(rgb),
	}
}

// ToolV2 is a tool definition the model may call. InputSchema is a JSON
// Schema object describing the tool's arguments.
type ToolV2 struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	InputSchema json.RawMessage `json:"input_schema"`
}

// RequestV2 is the v2 inference request envelope (ADR 0015). Populate
// ID and Messages; Attachments / Tools / sampling fields are optional.
//
// Pointer-typed sampling fields distinguish "omitted" (daemon applies
// the backend's default) from "explicitly zero", matching the v1
// Request convention.
type RequestV2 struct {
	ID          string         `json:"id,omitempty"`
	Messages    []MessageV2    `json:"messages"`
	Attachments []AttachmentV2 `json:"attachments,omitempty"`
	Tools       []ToolV2       `json:"tools,omitempty"`
	Temperature *float64       `json:"temperature,omitempty"`
	TopP        *float64       `json:"top_p,omitempty"`
	TopK        *uint32        `json:"top_k,omitempty"`
	MaxTokens   *uint32        `json:"max_tokens,omitempty"`
	Stream      *bool          `json:"stream,omitempty"`
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

// ErrorCodeV2 classifies v2 error frames (ADR 0015). Superset of the v1
// ErrorCode taxonomy.
type ErrorCodeV2 string

const (
	ErrV2QueueFull             ErrorCodeV2 = "queue_full"
	ErrV2BackendUnavailable    ErrorCodeV2 = "backend_unavailable"
	ErrV2InvalidRequest        ErrorCodeV2 = "invalid_request"
	ErrV2FrameTooLarge         ErrorCodeV2 = "frame_too_large"
	ErrV2Internal              ErrorCodeV2 = "internal"
	ErrV2AttachmentUnsupported ErrorCodeV2 = "attachment_unsupported"
	ErrV2ToolCallMalformed     ErrorCodeV2 = "tool_call_malformed"
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
