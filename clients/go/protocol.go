// Package inferd is the Go client for the inferd local-inference daemon.
//
// The wire protocol (NDJSON over UDS / Windows named pipe / loopback TCP)
// is documented in the inferd repo at docs/protocol-v1.md and is frozen
// per ADR 0008. This file defines the request and response shapes
// byte-compatible with that spec; the Client struct in client.go wraps
// the transport.
package inferd

// Role is the conversation role attached to a Message.
type Role string

const (
	RoleSystem    Role = "system"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
)

// Message is one conversation turn carried in Request.Messages.
type Message struct {
	Role    Role   `json:"role"`
	Content string `json:"content"`
}

// Request is the inference request envelope.
//
// Pointer-typed sampling fields distinguish "omitted" (server applies
// defaults) from "explicitly zero." See docs/protocol-v1.md §Request.
type Request struct {
	ID               string    `json:"id,omitempty"`
	Messages         []Message `json:"messages"`
	Temperature      *float64  `json:"temperature,omitempty"`
	TopP             *float64  `json:"top_p,omitempty"`
	TopK             *int      `json:"top_k,omitempty"`
	MaxTokens        *int      `json:"max_tokens,omitempty"`
	Stream           *bool     `json:"stream,omitempty"`
	ImageTokenBudget *int      `json:"image_token_budget,omitempty"`
	// Grammar is a llama.cpp GBNF string forwarded verbatim to the
	// backend. Empty / omitted means unconstrained generation.
	Grammar string `json:"grammar,omitempty"`
}

// ResponseType discriminates Response variants on the wire.
type ResponseType string

const (
	ResponseStatus ResponseType = "status"
	ResponseToken  ResponseType = "token"
	ResponseDone   ResponseType = "done"
	ResponseError  ResponseType = "error"
)

// StopReason on a done frame.
type StopReason string

const (
	StopEnd       StopReason = "end"
	StopLength    StopReason = "length"
	StopCancelled StopReason = "cancelled"
	StopError     StopReason = "error"
)

// ErrorCode classifies error frames per docs/protocol-v1.md and ADR 0008.
type ErrorCode string

const (
	ErrQueueFull          ErrorCode = "queue_full"
	ErrBackendUnavailable ErrorCode = "backend_unavailable"
	ErrInvalidRequest     ErrorCode = "invalid_request"
	ErrFrameTooLarge      ErrorCode = "frame_too_large"
	ErrInternal           ErrorCode = "internal"
)

// Usage is the token-count report carried on done frames.
type Usage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
}

// Response is one frame off the response stream. Variant is selected by
// Type; field set varies accordingly.
type Response struct {
	ID         string       `json:"id"`
	Type       ResponseType `json:"type"`
	Status     string       `json:"status,omitempty"`      // status only
	Content    string       `json:"content,omitempty"`     // token, done
	Usage      *Usage       `json:"usage,omitempty"`       // done
	StopReason StopReason   `json:"stop_reason,omitempty"` // done
	Backend    string       `json:"backend,omitempty"`     // done — diagnostic only
	Code       ErrorCode    `json:"code,omitempty"`        // error
	Message    string       `json:"message,omitempty"`     // error
}

// IsTerminal reports whether this frame ends a request stream.
func (r Response) IsTerminal() bool {
	return r.Type == ResponseDone || r.Type == ResponseError
}
