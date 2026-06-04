package inferd

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
)

// GenerateV2 sends one RequestV2 over the v2 wire surface and returns a
// channel of streamed ResponseV2 frames. The channel closes after the
// terminal frame (done or error) is delivered, or after ctx is
// cancelled.
//
// The v2 surface binds on a *separate* socket from v1 — dial it with
// the same DialUDS / DialPipe / DialTCP constructors pointed at the v2
// path (DefaultInferV2Addr returns the platform default). A Client
// dialled at the v1 socket cannot serve GenerateV2 and vice versa;
// they are distinct endpoints (ADR 0015).
//
// Cancellation, retry, and framing semantics match Generate: ctx
// cancel closes the connection (which cancels in-flight server-side
// generation per ADR 0007); the daemon never retries; frames are
// bounded at MaxFrameBytes.
func (c *Client) GenerateV2(ctx context.Context, req RequestV2) (<-chan ResponseV2, error) {
	if err := c.writeRequestV2(req); err != nil {
		return nil, err
	}

	out := make(chan ResponseV2, 8)
	go func() {
		defer close(out)
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
			resp, err := c.readResponseV2()
			if err != nil {
				if ctx.Err() != nil {
					select {
					case out <- ResponseV2{
						ID:      req.ID,
						Type:    ResponseV2Error,
						Code:    ErrV2Internal,
						Message: fmt.Sprintf("context cancelled: %v", ctx.Err()),
					}:
					default:
					}
				} else if !errors.Is(err, io.EOF) {
					select {
					case out <- ResponseV2{
						ID:      req.ID,
						Type:    ResponseV2Error,
						Code:    ErrV2Internal,
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

func (c *Client) writeRequestV2(req RequestV2) error {
	c.wMu.Lock()
	defer c.wMu.Unlock()
	if c.closed {
		return errors.New("inferd: client closed")
	}
	buf, err := json.Marshal(req)
	if err != nil {
		return fmt.Errorf("marshal v2 request: %w", err)
	}
	if len(buf) >= MaxFrameBytes {
		return fmt.Errorf("inferd: v2 request frame exceeds %d byte cap", MaxFrameBytes)
	}
	buf = append(buf, '\n')
	if _, err := c.conn.Write(buf); err != nil {
		return fmt.Errorf("write v2 request: %w", err)
	}
	return nil
}

func (c *Client) readResponseV2() (ResponseV2, error) {
	line, err := c.readLineCapped(MaxFrameBytes)
	if err != nil {
		return ResponseV2{}, err
	}
	var resp ResponseV2
	if err := json.Unmarshal(line, &resp); err != nil {
		return ResponseV2{}, fmt.Errorf("decode v2 response: %w", err)
	}
	return resp, nil
}
