package inferd

import (
	"os"
	"runtime"
)

// Default socket/pipe paths for the inference surfaces, mirroring the
// daemon's resolution chain. As of v0.4 (ADR 0021) generation is a
// single socket on a neutral path; embeddings (ADR 0017) and rerank
// (ADR 0027) keep their own:
//
//	generation   inferd.sock         \\.\pipe\inferd
//	embeddings   infer.embed.sock    \\.\pipe\inferd-infer-embed
//	rerank       infer.rerank.sock   \\.\pipe\inferd-infer-rerank
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

// DefaultInferAddr returns the platform default path for the generation
// socket (v0.4 / ADR 0021 — one generation socket on a neutral path).
// Dial it with DialUDS / DialPipe / DialTCP and call Client.GenerateV2.
func DefaultInferAddr() string {
	return runtimeSocketPath("inferd.sock", `\\.\pipe\inferd`)
}

// DefaultInferV2Addr is a deprecated alias for [DefaultInferAddr]. v0.4
// folded v1 into v2, so the "v2" generation socket is now simply the
// generation socket. Kept so existing callers keep compiling.
//
// Deprecated: use DefaultInferAddr.
func DefaultInferV2Addr() string {
	return DefaultInferAddr()
}

// DefaultInferEmbedAddr returns the platform default path for the
// embeddings socket (ADR 0017).
func DefaultInferEmbedAddr() string {
	return runtimeSocketPath("infer.embed.sock", `\\.\pipe\inferd-infer-embed`)
}

// DefaultInferRerankAddr returns the platform default path for the
// cross-encoder rerank socket (ADR 0027).
//
// The socket exists only when the warm model has a classification head,
// and one daemon serves one model (ADR 0012) — so on a host doing both
// retrieval and generation this path belongs to a different daemon
// process than [DefaultInferAddr].
func DefaultInferRerankAddr() string {
	return runtimeSocketPath("infer.rerank.sock", `\\.\pipe\inferd-infer-rerank`)
}
