//go:build unix

package inferd_test

import (
	"context"

	inferd "github.com/3rg0n/inferd/clients/go"
)

// dialTestInfer connects to the test daemon's per-test inference socket.
// On Unix that path is a UDS; the e2e test passes the same path it gave
// the daemon via --uds, rather than DialInfer's hardcoded platform
// default (which would point at the production socket name).
func dialTestInfer(ctx context.Context, addr string) (*inferd.Client, error) {
	return inferd.DialUDS(ctx, addr)
}
