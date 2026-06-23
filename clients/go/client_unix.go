//go:build unix

package inferd

import (
	"context"
	"fmt"
	"net"
)

// DialInfer opens a connection to the daemon's generation socket at the
// platform-default path. On Unix that is the UDS at DefaultInferAddr();
// on Windows the equivalent dials the named pipe. Use this for
// portable, transport-agnostic code; reach for DialUDS / DialPipe only
// when you need a non-default path.
func DialInfer(ctx context.Context) (*Client, error) {
	return DialUDS(ctx, DefaultInferAddr())
}

// DialUDS opens a Unix domain socket connection to the inferd daemon.
// Unix-only; on Windows use DialPipe.
func DialUDS(ctx context.Context, path string) (*Client, error) {
	var d net.Dialer
	conn, err := d.DialContext(ctx, "unix", path)
	if err != nil {
		return nil, fmt.Errorf("dial uds %s: %w", path, err)
	}
	return New(conn), nil
}

// dialAdminAddr is the platform-specific transport for the admin
// socket. On Unix this is always a UDS path.
func dialAdminAddr(ctx context.Context, addr string) (net.Conn, error) {
	var d net.Dialer
	return d.DialContext(ctx, "unix", addr)
}
