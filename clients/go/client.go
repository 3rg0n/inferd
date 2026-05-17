package inferd

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
)

// MaxFrameBytes is the per-frame cap inferd-proto enforces on both
// directions (THREAT_MODEL F-1; docs/protocol-v1.md §Framing).
const MaxFrameBytes = 64 << 20

// Client wraps one connection to the inferd daemon. Construct via
// DialTCP / DialUDS / DialPipe; close with Close.
//
// One Client serves one connection. Concurrent calls to Generate from
// multiple goroutines on the same Client are not supported in v0.1 —
// the daemon's wire protocol allows it (each frame carries an id), but
// this client doesn't yet multiplex. Open one Client per goroutine.
type Client struct {
	conn   net.Conn
	r      *bufio.Reader
	wMu    sync.Mutex
	closed bool
}

// New wraps an already-connected net.Conn. Useful for tests and for
// transports DialX doesn't cover.
func New(conn net.Conn) *Client {
	return &Client{
		conn: conn,
		r:    bufio.NewReaderSize(conn, 64*1024),
	}
}

// DialTCP opens a TCP connection to the inferd daemon.
func DialTCP(ctx context.Context, addr string) (*Client, error) {
	var d net.Dialer
	conn, err := d.DialContext(ctx, "tcp", addr)
	if err != nil {
		return nil, fmt.Errorf("dial tcp %s: %w", addr, err)
	}
	return New(conn), nil
}

// Close closes the underlying connection. Safe to call multiple times.
func (c *Client) Close() error {
	c.wMu.Lock()
	defer c.wMu.Unlock()
	if c.closed {
		return nil
	}
	c.closed = true
	return c.conn.Close()
}

// Generate sends one Request and returns a channel of streamed
// Responses. The channel closes after the terminal frame (done or
// error) is delivered, or after ctx is cancelled.
//
// On ctx cancel, the connection is closed which terminates any
// in-flight server-side generation per ADR 0007. Caller-side retries
// are the caller's responsibility — the daemon never retries.
func (c *Client) Generate(ctx context.Context, req Request) (<-chan Response, error) {
	if err := c.writeRequest(req); err != nil {
		return nil, err
	}

	out := make(chan Response, 8)
	go func() {
		defer close(out)
		// Watch ctx in parallel with the read loop so a Done channel
		// signal closes the connection — the Read in readResponse
		// then unblocks with an error and the goroutine exits.
		stopCh := make(chan struct{})
		go func() {
			select {
			case <-ctx.Done():
				_ = c.conn.Close()
			case <-stopCh:
			}
		}()
		defer close(stopCh)

		for {
			resp, err := c.readResponse()
			if err != nil {
				// Surface ctx errors as a synthetic local error frame
				// so callers don't have to consult ctx.Err separately.
				if ctx.Err() != nil {
					select {
					case out <- Response{
						ID:      req.ID,
						Type:    ResponseError,
						Code:    ErrInternal,
						Message: fmt.Sprintf("context cancelled: %v", ctx.Err()),
					}:
					default:
					}
				} else if !errors.Is(err, io.EOF) {
					select {
					case out <- Response{
						ID:      req.ID,
						Type:    ResponseError,
						Code:    ErrInternal,
						Message: fmt.Sprintf("read: %v", err),
					}:
					default:
					}
				}
				return
			}
			terminal := resp.IsTerminal()
			out <- resp
			if terminal {
				return
			}
		}
	}()
	return out, nil
}

func (c *Client) writeRequest(req Request) error {
	c.wMu.Lock()
	defer c.wMu.Unlock()
	if c.closed {
		return errors.New("inferd: client closed")
	}
	buf, err := json.Marshal(req)
	if err != nil {
		return fmt.Errorf("marshal request: %w", err)
	}
	if len(buf) >= MaxFrameBytes {
		return fmt.Errorf("inferd: request frame exceeds %d byte cap", MaxFrameBytes)
	}
	buf = append(buf, '\n')
	if _, err := c.conn.Write(buf); err != nil {
		return fmt.Errorf("write request: %w", err)
	}
	return nil
}

func (c *Client) readResponse() (Response, error) {
	// Bounded line read — the bufio.Reader's internal buffer grows up
	// to MaxFrameBytes; anything larger is treated as the same protocol
	// violation the daemon emits a frame_too_large error for.
	line, err := c.readLineCapped(MaxFrameBytes)
	if err != nil {
		return Response{}, err
	}
	var resp Response
	if err := json.Unmarshal(line, &resp); err != nil {
		return Response{}, fmt.Errorf("decode response: %w", err)
	}
	return resp, nil
}

// readLineCapped reads up to (and including) the next '\n', refusing
// at limit bytes without a newline. Mirrors inferd-proto's bounded
// reader (THREAT_MODEL F-1).
func (c *Client) readLineCapped(limit int) ([]byte, error) {
	var line []byte
	for {
		chunk, err := c.r.ReadSlice('\n')
		if err == nil {
			line = append(line, chunk...)
			if len(line) > limit {
				return nil, fmt.Errorf("inferd: response frame exceeds %d byte cap", limit)
			}
			return line, nil
		}
		if errors.Is(err, bufio.ErrBufferFull) {
			line = append(line, chunk...)
			if len(line) > limit {
				return nil, fmt.Errorf("inferd: response frame exceeds %d byte cap", limit)
			}
			continue
		}
		// EOF or transport error.
		if len(chunk) > 0 {
			line = append(line, chunk...)
			return line, err
		}
		return nil, err
	}
}
