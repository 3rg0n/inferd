package inferd

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"sync"
)

// MaxFrameBytes is the per-frame cap inferd-proto enforces on both
// directions (THREAT_MODEL F-1 / F-5). On the generation surface the
// cap applies to a length-prefixed frame's payload; the embed surface
// keeps the NDJSON line cap.
const MaxFrameBytes = 64 << 20

// Client wraps one connection to the inferd daemon. Construct via
// DialTCP / DialUDS / DialPipe; close with Close. Generation calls go
// through GenerateV2 (client_v2.go) over the length-prefixed wire.
//
// One Client serves one connection. Concurrent generation calls from
// multiple goroutines on the same Client are not supported — the
// daemon's wire allows it (each frame carries an id), but this client
// doesn't multiplex. Open one Client per goroutine.
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
//
// DEPRECATED (ADR 0022): the daemon's inbound loopback-TCP listener is
// deprecated in v0.4.0 and will be removed in v0.4.1. New code should
// dial the local UDS / named pipe via DialInfer / DialUDS / DialPipe.
// For network access, use the separate inferd-http bridge (ADR 0020).
// This constructor is retained for the v0.4.x test harness only.
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
