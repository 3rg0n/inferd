package inferd

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"runtime"
	"time"
)

// AdminClient subscribes to the inferd admin socket per the spec at
// docs/protocol-v1.md §"Admin endpoint". Read-only stream of
// lifecycle events. Use this when you need progress UX during the
// first-boot model download or want to watch for the daemon entering
// the `ready` state explicitly. For most middleware,
// DialAndWaitReady is simpler and sufficient.
type AdminClient struct {
	conn net.Conn
	r    *bufio.Reader
}

// AdminEvent is one parsed admin frame. Field set varies by Status
// and (for `loading_model`) Phase. Unknown keys are surfaced via the
// Extra map so consumers can survive forward-compatible additions to
// the wire format per the spec's MUST-IGNORE rule.
type AdminEvent struct {
	// ID is always "admin" on this channel.
	ID string `json:"id"`
	// Type is always "status" in v1.
	Type string `json:"type"`
	// Status is one of: starting, loading_model, ready, restarting, draining.
	// Unknown values land here verbatim — consumers MUST ignore them.
	Status string `json:"status"`
	// Phase is set on `loading_model` and `restarting`. One of:
	// checking_local, download, verify, quarantine, mmap, kv_cache.
	// Unknown values land here verbatim.
	Phase string `json:"phase,omitempty"`

	// Phase-specific detail keys, flattened on the wire per the spec.
	Path            string `json:"path,omitempty"`
	DownloadedBytes int64  `json:"downloaded_bytes,omitempty"`
	TotalBytes      *int64 `json:"total_bytes,omitempty"` // pointer: null = unknown
	SourceURL       string `json:"source_url,omitempty"`
	ExpectedSHA256  string `json:"expected_sha256,omitempty"`
	ActualSHA256    string `json:"actual_sha256,omitempty"`
	QuarantinePath  string `json:"quarantine_path,omitempty"`
	NCtx            int    `json:"n_ctx,omitempty"`

	// Capability keys, set on `status: "capabilities"` frames (one per
	// registered backend; the daemon emits these on connect before the
	// lifecycle snapshot). Pointer-typed so absence (any non-capabilities
	// frame) is distinguishable from an explicit false. Consumers gate
	// multimodal dispatch on Vision/Audio: dial the v2 socket and send
	// image/audio attachments only when the relevant capability is true.
	Backend  string `json:"backend,omitempty"`
	V2       *bool  `json:"v2,omitempty"`
	Vision   *bool  `json:"vision,omitempty"`
	Audio    *bool  `json:"audio,omitempty"`
	Tools    *bool  `json:"tools,omitempty"`
	Thinking *bool  `json:"thinking,omitempty"`
	Embed    *bool  `json:"embed,omitempty"`

	// AudioSampleRate is the rate in Hz that audio attachments MUST use
	// when Audio is true. The model's audio encoder takes no rate
	// parameter — it consumes samples at the rate it was trained for —
	// so the daemon rejects any other rate rather than silently
	// time-scaling the audio. Resample to this value before sending; the
	// daemon does not resample.
	AudioSampleRate *uint32 `json:"audio_sample_rate,omitempty"`

	// Extra holds any keys we don't recognise. Per the spec, clients
	// MUST ignore unknown keys without erroring; surfacing them here
	// lets diagnostic-curious consumers display them anyway.
	Extra map[string]json.RawMessage `json:"-"`
}

// IsCapabilities reports whether this frame is a backend capability
// advertisement (the daemon emits one per registered backend on
// connect, ahead of the lifecycle snapshot).
func (e AdminEvent) IsCapabilities() bool {
	return e.Status == "capabilities"
}

// SupportsVision reports whether this capabilities frame advertises
// vision support. False for non-capabilities frames or when the field
// is absent.
func (e AdminEvent) SupportsVision() bool {
	return e.Vision != nil && *e.Vision
}

// SupportsAudio reports whether this capabilities frame advertises audio
// support. False for non-capabilities frames or when the field is absent.
func (e AdminEvent) SupportsAudio() bool {
	return e.Audio != nil && *e.Audio
}

// RequiredAudioSampleRate returns the sample rate in Hz that audio
// attachments must use, and whether the frame advertised one. Send audio
// only at this rate: the daemon rejects a mismatch rather than resampling
// (a rate the encoder wasn't trained for time-scales the audio and yields
// a plausible wrong answer).
func (e AdminEvent) RequiredAudioSampleRate() (uint32, bool) {
	if e.AudioSampleRate == nil {
		return 0, false
	}
	return *e.AudioSampleRate, true
}

// DialAdmin opens a read-only connection to the inferd admin socket
// and returns an AdminClient. The default platform-appropriate path
// is used unless `addr` is non-empty.
//
// On Unix `addr` is a UDS path. On Windows `addr` is a named-pipe
// path (e.g. `\\.\pipe\inferd-admin`).
//
// The first frame on the connection is a snapshot of the daemon's
// current state. Subsequent frames push as the daemon transitions.
func DialAdmin(ctx context.Context, addr string) (*AdminClient, error) {
	if addr == "" {
		addr = DefaultAdminAddr()
	}
	conn, err := dialAdminAddr(ctx, addr)
	if err != nil {
		return nil, fmt.Errorf("dial admin %s: %w", addr, err)
	}
	return &AdminClient{
		conn: conn,
		r:    bufio.NewReaderSize(conn, 64*1024),
	}, nil
}

// DefaultAdminAddr returns the platform-appropriate default admin
// socket path per docs/protocol-v1.md §"Admin endpoint".
//
// Linux resolution chain:
//  1. $XDG_RUNTIME_DIR/inferd/admin.sock (set by systemd-logind on
//     session start; the per-user equivalent of /run/<svc>/).
//  2. $HOME/.inferd/run/admin.sock for sessions without logind
//     (containers, ssh without a real login session).
//  3. /tmp/inferd/admin.sock as a last resort.
func DefaultAdminAddr() string {
	switch runtime.GOOS {
	case "linux":
		if xdg := os.Getenv("XDG_RUNTIME_DIR"); xdg != "" {
			return xdg + "/inferd/admin.sock"
		}
		if home := os.Getenv("HOME"); home != "" {
			return home + "/.inferd/run/admin.sock"
		}
		return "/tmp/inferd/admin.sock"
	case "darwin":
		// macOS: ${TMPDIR}/inferd/admin.sock per the spec.
		// Go's os.TempDir() returns the same.
		return tempDir() + "/inferd/admin.sock"
	case "windows":
		return `\\.\pipe\inferd-admin`
	default:
		return "/tmp/inferd/admin.sock"
	}
}

// Close closes the underlying connection.
func (a *AdminClient) Close() error {
	if a.conn == nil {
		return nil
	}
	err := a.conn.Close()
	a.conn = nil
	return err
}

// Recv reads the next admin event. Blocks until a frame arrives, the
// daemon EOFs, or ctx cancels (in which case the connection is
// closed and a context error is returned).
//
// Forward compatibility: per the spec, unknown `Status` and `Phase`
// values are surfaced verbatim. Callers MUST handle them by ignoring
// or logging — never by erroring.
func (a *AdminClient) Recv(ctx context.Context) (AdminEvent, error) {
	if a.conn == nil {
		return AdminEvent{}, errors.New("inferd: admin client closed")
	}

	// Watch ctx in a goroutine; close the connection if it cancels so
	// the bufio Read returns.
	stop := make(chan struct{})
	go func() {
		select {
		case <-ctx.Done():
			_ = a.conn.Close()
		case <-stop:
		}
	}()
	defer close(stop)

	line, err := a.readLineCapped(MaxFrameBytes)
	if err != nil {
		if ctx.Err() != nil {
			return AdminEvent{}, ctx.Err()
		}
		return AdminEvent{}, err
	}
	var ev AdminEvent
	if err := json.Unmarshal(line, &ev); err != nil {
		return AdminEvent{}, fmt.Errorf("decode admin event: %w", err)
	}
	// Sanity: id should always be "admin" in v1. Older or future
	// daemons that don't honour this still work — we don't enforce.
	return ev, nil
}

// WaitReady blocks until the daemon publishes a `ready` event, then
// returns the snapshot AdminEvent that flipped state. Returns ctx.Err
// if cancelled, io.EOF if the daemon closes the connection before
// reaching ready (a daemon that crashes mid-load looks like this).
//
// During a multi-GB first-boot download this blocks for the entire
// download; callers can poll AdminClient.Recv themselves to display
// progress along the way.
func (a *AdminClient) WaitReady(ctx context.Context) (AdminEvent, error) {
	for {
		ev, err := a.Recv(ctx)
		if err != nil {
			return AdminEvent{}, err
		}
		if ev.Status == "ready" {
			return ev, nil
		}
	}
}

// readLineCapped reads up to (and including) the next '\n', refusing
// past `limit` bytes. Mirrors inferd-proto's bounded reader (F-1).
func (a *AdminClient) readLineCapped(limit int) ([]byte, error) {
	var line []byte
	for {
		chunk, err := a.r.ReadSlice('\n')
		if err == nil {
			line = append(line, chunk...)
			if len(line) > limit {
				return nil, fmt.Errorf("inferd: admin frame exceeds %d byte cap", limit)
			}
			return line, nil
		}
		if errors.Is(err, bufio.ErrBufferFull) {
			line = append(line, chunk...)
			if len(line) > limit {
				return nil, fmt.Errorf("inferd: admin frame exceeds %d byte cap", limit)
			}
			continue
		}
		if len(chunk) > 0 {
			line = append(line, chunk...)
			return line, err
		}
		return nil, err
	}
}

// DialAndWaitReady is the all-in-one helper for the Pattern A
// passive readiness check from docs/protocol-v1.md §"Client
// connection lifecycle". Retries connect against the inference
// transport with exponential backoff (start 100ms, cap 5s) until
// success or the deadline carried by ctx.
//
// Most middleware that wants "wait for inferd to come up" should
// call this rather than reaching for the admin socket. The inference
// socket only exists when the daemon is `ready` (THREAT_MODEL F-13);
// successful connect is itself the ready signal.
//
// `dial` is the dialler matching the transport (usually a closure
// over DialTCP / DialUDS / DialPipe with the right address). The
// helper takes a closure rather than baking in a transport so
// callers can pick UDS / pipe / TCP without us duplicating the
// retry loop per transport.
func DialAndWaitReady(
	ctx context.Context,
	dial func(context.Context) (*Client, error),
) (*Client, error) {
	const (
		initial = 100 * time.Millisecond
		max     = 5 * time.Second
	)

	delay := initial
	for {
		client, err := dial(ctx)
		if err == nil {
			return client, nil
		}
		if !isTransientDialError(err) {
			return nil, err
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(delay):
		}
		delay *= 2
		if delay > max {
			delay = max
		}
	}
}

// isTransientDialError reports whether `err` is the kind of failure
// the daemon's F-13 ready-gating produces during bring-up, i.e.
// "socket not yet bound." Other errors (permission denied, malformed
// addr) are not transient and bubble up immediately.
func isTransientDialError(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	// We pattern-match strings rather than syscall.Errno values
	// because the same condition surfaces with different OS error
	// messages: ECONNREFUSED on Linux, ENOENT for an absent UDS,
	// ERROR_FILE_NOT_FOUND / ERROR_PIPE_BUSY on Windows.
	transientFragments := []string{
		"connection refused",
		"no such file",
		"cannot find the file",
		"all pipe instances are busy",
		"the system cannot find",
		"operation timed out",
		"target machine actively refused",
	}
	for _, frag := range transientFragments {
		if containsFold(msg, frag) {
			return true
		}
	}
	return false
}

// containsFold is a tiny case-insensitive substring search; we don't
// pull in strings.EqualFold here because we're scanning small fixed
// fragments against arbitrary OS messages.
func containsFold(haystack, needle string) bool {
	if len(needle) == 0 {
		return true
	}
	if len(haystack) < len(needle) {
		return false
	}
	hl := []byte(haystack)
	nl := []byte(needle)
	for i := 0; i+len(nl) <= len(hl); i++ {
		match := true
		for j := 0; j < len(nl); j++ {
			a := hl[i+j]
			b := nl[j]
			if a >= 'A' && a <= 'Z' {
				a += 'a' - 'A'
			}
			if b >= 'A' && b <= 'Z' {
				b += 'a' - 'A'
			}
			if a != b {
				match = false
				break
			}
		}
		if match {
			return true
		}
	}
	return false
}

func tempDir() string {
	if d := os.Getenv("TMPDIR"); d != "" {
		return d
	}
	return "/tmp"
}
