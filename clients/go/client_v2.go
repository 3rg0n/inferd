package inferd

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
)

// WireVersion is the wire-format version this client speaks (ADR 0021).
// Set on every RequestV2 and checked by the daemon against its own; a
// mismatch comes back as a wire_version_unsupported error frame.
const WireVersion uint32 = 1

// frame-type tags for the length-prefixed framing (ADR 0021).
const (
	frameJSON byte = 0x01
	frameBlob byte = 0x02
)

// GenerateV2 sends one RequestV2 over the v2 generation socket and
// returns a channel of streamed ResponseV2 frames. The channel closes
// after the terminal frame (done or error), or after ctx is cancelled.
//
// As of v0.4 (ADR 0021) the wire is length-prefixed and type-tagged
// ([uvarint len][1 byte type][payload]); a request carrying attachments
// sends the request JSON frame, then per attachment a BlobDescriptor
// JSON frame followed by a BLOB frame with the raw bytes (no base64).
// The client sets RequestV2.WireVersion automatically.
//
// Dial the generation socket with DialUDS / DialPipe / DialTCP
// (DefaultInferAddr returns the platform default). Cancellation, retry,
// and the 64 MiB frame cap match Generate.
func (c *Client) GenerateV2(ctx context.Context, req RequestV2) (<-chan ResponseV2, error) {
	req.WireVersion = WireVersion
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
			ftype, payload, err := c.readFrame()
			if err != nil {
				if ctx.Err() != nil {
					emitV2Err(out, req.ID, fmt.Sprintf("context cancelled: %v", ctx.Err()))
				} else if !errors.Is(err, io.EOF) {
					emitV2Err(out, req.ID, fmt.Sprintf("read: %v", err))
				}
				return
			}
			if ftype != frameJSON {
				emitV2Err(out, req.ID, "daemon sent a non-JSON frame on the response stream")
				return
			}
			var resp ResponseV2
			if err := json.Unmarshal(payload, &resp); err != nil {
				emitV2Err(out, req.ID, fmt.Sprintf("decode v2 response: %v", err))
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

func emitV2Err(out chan<- ResponseV2, id, msg string) {
	select {
	case out <- ResponseV2{ID: id, Type: ResponseV2Error, Code: ErrV2Internal, Message: msg}:
	default:
	}
}

// writeRequestV2 writes the request JSON frame followed by one
// (descriptor, BLOB) pair per attachment that carries bytes.
func (c *Client) writeRequestV2(req RequestV2) error {
	c.wMu.Lock()
	defer c.wMu.Unlock()
	if c.closed {
		return errors.New("inferd: client closed")
	}

	// Collect attachment bytes; they ride in BLOB frames, not the JSON.
	type blob struct {
		id    string
		bytes []byte
	}
	var blobs []blob
	for _, a := range req.Attachments {
		if len(a.Bytes) > 0 {
			blobs = append(blobs, blob{id: a.ID, bytes: a.Bytes})
		}
	}

	body, err := json.Marshal(req)
	if err != nil {
		return fmt.Errorf("marshal v2 request: %w", err)
	}
	if err := c.writeFrame(frameJSON, body); err != nil {
		return err
	}
	for _, b := range blobs {
		desc, err := json.Marshal(BlobDescriptor{
			Type:         "attachment_blob",
			AttachmentID: b.id,
			Len:          uint64(len(b.bytes)),
		})
		if err != nil {
			return fmt.Errorf("marshal blob descriptor: %w", err)
		}
		if err := c.writeFrame(frameJSON, desc); err != nil {
			return err
		}
		if err := c.writeFrame(frameBlob, b.bytes); err != nil {
			return err
		}
	}
	return nil
}

// writeFrame writes one length-prefixed, type-tagged frame.
func (c *Client) writeFrame(ftype byte, payload []byte) error {
	if len(payload) > MaxFrameBytes {
		return fmt.Errorf("inferd: frame exceeds %d byte cap", MaxFrameBytes)
	}
	var prefix [binary.MaxVarintLen64]byte
	n := binary.PutUvarint(prefix[:], uint64(len(payload)))
	if _, err := c.conn.Write(prefix[:n]); err != nil {
		return fmt.Errorf("write frame length: %w", err)
	}
	if _, err := c.conn.Write([]byte{ftype}); err != nil {
		return fmt.Errorf("write frame type: %w", err)
	}
	if _, err := c.conn.Write(payload); err != nil {
		return fmt.Errorf("write frame payload: %w", err)
	}
	return nil
}

// readFrame reads one length-prefixed, type-tagged frame: the type byte
// and the raw payload. The 64 MiB cap is enforced on the length before
// the payload is read.
func (c *Client) readFrame() (byte, []byte, error) {
	length, err := binary.ReadUvarint(c.r)
	if err != nil {
		return 0, nil, err // io.EOF between frames bubbles up
	}
	if length > uint64(MaxFrameBytes) {
		return 0, nil, fmt.Errorf("inferd: frame length %d exceeds %d byte cap", length, MaxFrameBytes)
	}
	ftype, err := c.r.ReadByte()
	if err != nil {
		return 0, nil, err
	}
	if ftype != frameJSON && ftype != frameBlob {
		return 0, nil, fmt.Errorf("inferd: unknown frame-type byte 0x%02x", ftype)
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(c.r, payload); err != nil {
		return 0, nil, err
	}
	return ftype, payload, nil
}
