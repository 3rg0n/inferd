// Package inferd is the Go client for the inferd local-inference daemon.
//
// As of v0.4 (ADR 0021) the daemon exposes a single generation surface
// (v2) on the length-prefixed, type-tagged wire plus an embeddings
// surface; the original text-only v1 NDJSON generation wire was folded
// into v2 and removed. The v2 request/response shapes live in
// protocol_v2.go and the GenerateV2 transport in client_v2.go. This
// file holds the small set of types shared across surfaces.
package inferd

// Role is the conversation role attached to a message. Shared by the v2
// MessageV2 type (protocol_v2.go).
type Role string

const (
	RoleSystem    Role = "system"
	RoleUser      Role = "user"
	RoleAssistant Role = "assistant"
)
