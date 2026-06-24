//go:build windows

package inferd_test

import (
	"context"

	inferd "github.com/3rg0n/inferd/clients/go"
)

// dialTestInfer connects to the test daemon's per-test inference socket.
// On Windows that path is a named pipe; the e2e test passes the same path
// it gave the daemon via --pipe, rather than DialInfer's hardcoded
// platform default (which would point at the production pipe name).
func dialTestInfer(ctx context.Context, addr string) (*inferd.Client, error) {
	return inferd.DialPipe(ctx, addr)
}
