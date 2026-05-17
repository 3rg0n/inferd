//go:build windows

package inferd

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"time"
)

// DialPipe opens a Windows named-pipe connection to the inferd daemon.
//
// Windows-only. v0.1 uses os.OpenFile against the pipe path with a
// short retry loop covering the "all instances are busy" window
// between the daemon accepting one client and binding the next
// instance.
//
// For richer pipe semantics (overlapping I/O, cancellation,
// remote pipes), consider importing github.com/Microsoft/go-winio
// in your application; this client deliberately stays
// dependency-free for v0.1.
func DialPipe(ctx context.Context, path string) (*Client, error) {
	deadline := time.Now().Add(10 * time.Second)
	if d, ok := ctx.Deadline(); ok && d.Before(deadline) {
		deadline = d
	}

	for {
		f, err := os.OpenFile(path, os.O_RDWR, 0)
		if err == nil {
			return New(&pipeConn{File: f}), nil
		}
		// "All pipe instances are busy" — retry briefly.
		if isPipeBusy(err) && time.Now().Before(deadline) {
			select {
			case <-ctx.Done():
				return nil, ctx.Err()
			case <-time.After(20 * time.Millisecond):
			}
			continue
		}
		return nil, fmt.Errorf("open pipe %s: %w", path, err)
	}
}

func isPipeBusy(err error) bool {
	// ERROR_PIPE_BUSY = 231. We compare on the underlying error string
	// to avoid pulling in golang.org/x/sys for a single check.
	return err != nil && (errors.Is(err, os.ErrNotExist) ||
		containsErrPipeBusy(err.Error()))
}

func containsErrPipeBusy(s string) bool {
	return s != "" &&
		(stringContains(s, "all pipe instances are busy") ||
			stringContains(s, "The system cannot find the file") ||
			stringContains(s, "Access is denied"))
}

func stringContains(haystack, needle string) bool {
	return len(haystack) >= len(needle) && indexOf(haystack, needle) >= 0
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

// pipeConn adapts an os.File to net.Conn so the Client uses the same
// machinery for TCP and pipes.
type pipeConn struct {
	*os.File
}

func (p *pipeConn) LocalAddr() net.Addr  { return pipeAddr(p.Name()) }
func (p *pipeConn) RemoteAddr() net.Addr { return pipeAddr(p.Name()) }

func (p *pipeConn) SetDeadline(t time.Time) error      { return p.File.SetDeadline(t) }
func (p *pipeConn) SetReadDeadline(t time.Time) error  { return p.File.SetReadDeadline(t) }
func (p *pipeConn) SetWriteDeadline(t time.Time) error { return p.File.SetWriteDeadline(t) }

type pipeAddr string

func (pipeAddr) Network() string  { return "pipe" }
func (a pipeAddr) String() string { return string(a) }
