//go:build unix

package inferd

import (
	"context"
	"fmt"
	"net"
)

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
