package inferd

import (
	"os"
	"runtime"
)

// Default socket/pipe paths for the inference surfaces, mirroring the
// daemon's resolution chain (docs/protocol-v1.md §"Default endpoint
// resolution" for v1; ADR 0015 / 0017 for the v2 / embed paths). Each
// wire surface binds on its own socket:
//
//	v1 generation  infer.sock        \\.\pipe\inferd-infer
//	v2 generation  infer.v2.sock     \\.\pipe\inferd-infer-v2
//	embeddings     infer.embed.sock  \\.\pipe\inferd-infer-embed
//
// On Unix the resolution chain is:
//  1. $XDG_RUNTIME_DIR/inferd/<name> (set by systemd-logind)
//  2. $HOME/.inferd/run/<name> (sessions without logind)
//  3. /tmp/inferd/<name> (last resort)
//
// On macOS, ${TMPDIR}/inferd/<name>. On Windows, the named pipe.
func runtimeSocketPath(unixName, windowsPipe string) string {
	switch runtime.GOOS {
	case "windows":
		return windowsPipe
	case "darwin":
		return tempDir() + "/inferd/" + unixName
	default: // linux and other unix
		if xdg := os.Getenv("XDG_RUNTIME_DIR"); xdg != "" {
			return xdg + "/inferd/" + unixName
		}
		if home := os.Getenv("HOME"); home != "" {
			return home + "/.inferd/run/" + unixName
		}
		return "/tmp/inferd/" + unixName
	}
}

// DefaultInferAddr returns the platform default path for the v1
// generation socket.
func DefaultInferAddr() string {
	return runtimeSocketPath("infer.sock", `\\.\pipe\inferd-infer`)
}

// DefaultInferV2Addr returns the platform default path for the v2
// generation socket (ADR 0015). Dial it with DialUDS / DialPipe and
// call Client.GenerateV2.
func DefaultInferV2Addr() string {
	return runtimeSocketPath("infer.v2.sock", `\\.\pipe\inferd-infer-v2`)
}

// DefaultInferEmbedAddr returns the platform default path for the
// embeddings socket (ADR 0017).
func DefaultInferEmbedAddr() string {
	return runtimeSocketPath("infer.embed.sock", `\\.\pipe\inferd-infer-embed`)
}
