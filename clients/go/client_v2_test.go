package inferd_test

import (
	"bufio"
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net"
	"testing"
	"time"

	inferd "github.com/3rg0n/inferd/clients/go"
)

const (
	tFrameJSON byte = 0x01
	tFrameBlob byte = 0x02
)

// srvReadFrame reads one length-prefixed, type-tagged frame on the test
// server side, mirroring the client's writer (ADR 0021).
func srvReadFrame(t *testing.T, r *bufio.Reader) (byte, []byte) {
	t.Helper()
	length, err := binary.ReadUvarint(r)
	if err != nil {
		t.Errorf("read frame length: %v", err)
		return 0, nil
	}
	ftype, err := r.ReadByte()
	if err != nil {
		t.Errorf("read frame type: %v", err)
		return 0, nil
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(r, payload); err != nil {
		t.Errorf("read frame payload: %v", err)
		return 0, nil
	}
	return ftype, payload
}

func srvWriteJSONFrame(w net.Conn, v any) {
	body, _ := json.Marshal(v)
	var prefix [binary.MaxVarintLen64]byte
	n := binary.PutUvarint(prefix[:], uint64(len(body)))
	_, _ = w.Write(prefix[:n])
	_, _ = w.Write([]byte{tFrameJSON})
	_, _ = w.Write(body)
}

// TestV2MultimodalRoundTrip exercises the v2 length-prefixed framing
// (ADR 0021): a text + image-attachment request in (request JSON frame
// + blob descriptor frame + raw BLOB frame), a streamed text frame +
// done out. Proves the Go client's wire bytes match what the daemon
// expects, without needing the daemon binary (issue #31, #34).
func TestV2MultimodalRoundTrip(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer ln.Close()

	srvDone := make(chan struct{})
	go func() {
		defer close(srvDone)
		conn, err := ln.Accept()
		if err != nil {
			t.Logf("accept: %v", err)
			return
		}
		defer conn.Close()
		br := bufio.NewReader(conn)

		// 1. Request JSON frame.
		ftype, payload := srvReadFrame(t, br)
		if ftype != tFrameJSON {
			t.Errorf("request frame type = 0x%02x, want JSON", ftype)
			return
		}
		var got inferd.RequestV2
		if err := json.Unmarshal(payload, &got); err != nil {
			t.Errorf("decode v2 request: %v", err)
			return
		}
		if got.WireVersion != inferd.WireVersion {
			t.Errorf("wire_version = %d, want %d", got.WireVersion, inferd.WireVersion)
		}
		if got.ID != "v2-1" || len(got.Messages) != 1 {
			t.Errorf("unexpected request envelope: %+v", got)
		}
		msg := got.Messages[0]
		if msg.Role != inferd.RoleUser || len(msg.Content) != 2 {
			t.Errorf("unexpected message: %+v", msg)
		}
		if msg.Content[0].Type != inferd.ContentText || msg.Content[0].Text == "" {
			t.Errorf("content[0] should be non-empty text: %+v", msg.Content[0])
		}
		if msg.Content[1].Type != inferd.ContentImage || msg.Content[1].AttachmentID != "img" {
			t.Errorf("content[1] should reference image attachment: %+v", msg.Content[1])
		}
		if len(got.Attachments) != 1 {
			t.Errorf("expected 1 attachment, got %d", len(got.Attachments))
			return
		}
		att := got.Attachments[0]
		// Bytes are out-of-band (json:"-"), so the request JSON has none.
		if att.Kind != inferd.AttachmentImage || att.ID != "img" ||
			att.Width != 2 || att.Height != 2 || len(att.Bytes) != 0 {
			t.Errorf("unexpected attachment: %+v", att)
		}

		// 2. Blob descriptor JSON frame.
		ftype, payload = srvReadFrame(t, br)
		if ftype != tFrameJSON {
			t.Errorf("descriptor frame type = 0x%02x, want JSON", ftype)
			return
		}
		var desc inferd.BlobDescriptor
		if err := json.Unmarshal(payload, &desc); err != nil {
			t.Errorf("decode blob descriptor: %v", err)
			return
		}
		if desc.Type != "attachment_blob" || desc.AttachmentID != "img" || desc.Len != 12 {
			t.Errorf("unexpected descriptor: %+v", desc)
		}

		// 3. BLOB frame with the raw 2x2x3 RGB bytes.
		ftype, blob := srvReadFrame(t, br)
		if ftype != tFrameBlob {
			t.Errorf("blob frame type = 0x%02x, want BLOB", ftype)
			return
		}
		if uint64(len(blob)) != desc.Len {
			t.Errorf("blob len %d != descriptor %d", len(blob), desc.Len)
		}

		// Reply with one text frame and one done.
		srvWriteJSONFrame(conn, inferd.ResponseV2{
			ID: "v2-1", Type: inferd.ResponseV2Frame,
			Block: &inferd.ResponseBlockV2{Type: inferd.BlockText, Delta: "a red"},
		})
		srvWriteJSONFrame(conn, inferd.ResponseV2{
			ID: "v2-1", Type: inferd.ResponseV2Done,
			Usage:      &inferd.UsageV2{InputTokens: 276, OutputTokens: 2},
			StopReason: inferd.StopEndTurn,
			Backend:    "llamacpp",
		})
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	client, err := inferd.DialTCP(ctx, ln.Addr().String())
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer client.Close()

	// 2x2 RGB: a tiny image. Consumer-decoded raw bytes per ADR 0016.
	rgb := []byte{
		220, 30, 30, 255, 255, 255,
		255, 255, 255, 220, 30, 30,
	}
	stream, err := client.GenerateV2(ctx, inferd.RequestV2{
		ID: "v2-1",
		Messages: []inferd.MessageV2{{
			Role: inferd.RoleUser,
			Content: []inferd.ContentBlock{
				inferd.TextBlock("What color is in this image?"),
				inferd.ImageBlock("img"),
			},
		}},
		Attachments: []inferd.AttachmentV2{
			inferd.ImageAttachment("img", 2, 2, rgb),
		},
	})
	if err != nil {
		t.Fatalf("generate_v2: %v", err)
	}

	var frames []inferd.ResponseV2
	var text string
	for f := range stream {
		frames = append(frames, f)
		if f.Type == inferd.ResponseV2Frame && f.Block != nil && f.Block.Type == inferd.BlockText {
			text += f.Block.Delta
		}
	}
	<-srvDone

	if len(frames) != 2 {
		t.Fatalf("expected 2 frames, got %d: %+v", len(frames), frames)
	}
	if text != "a red" {
		t.Errorf("reassembled text = %q, want %q", text, "a red")
	}
	last := frames[len(frames)-1]
	if !last.IsTerminal() || last.Type != inferd.ResponseV2Done {
		t.Errorf("last frame not a done: %+v", last)
	}
	if last.Backend != "llamacpp" || last.StopReason != inferd.StopEndTurn {
		t.Errorf("done frame fields unexpected: %+v", last)
	}
	if last.Usage == nil || last.Usage.InputTokens != 276 {
		t.Errorf("usage unexpected: %+v", last.Usage)
	}
}

// TestCapabilitiesFrameDecode proves the AdminEvent capability fields
// decode from a `status: "capabilities"` frame and that SupportsVision
// reflects a vision-capable backend (issue #31 — consumers gate
// multimodal dispatch on this).
func TestCapabilitiesFrameDecode(t *testing.T) {
	line := []byte(`{"id":"admin","type":"status","status":"capabilities",` +
		`"backend":"gemma-4-e4b","v2":true,"vision":true,"audio":true,` +
		`"tools":true,"thinking":true,"embed":false}`)
	var ev inferd.AdminEvent
	if err := json.Unmarshal(line, &ev); err != nil {
		t.Fatalf("decode capabilities frame: %v", err)
	}
	if !ev.IsCapabilities() {
		t.Errorf("IsCapabilities() = false, want true")
	}
	if ev.Backend != "gemma-4-e4b" {
		t.Errorf("backend = %q", ev.Backend)
	}
	if !ev.SupportsVision() {
		t.Errorf("SupportsVision() = false, want true")
	}
	if ev.V2 == nil || !*ev.V2 || ev.Audio == nil || !*ev.Audio {
		t.Errorf("v2/audio caps not decoded: %+v", ev)
	}
	if ev.Embed == nil || *ev.Embed {
		t.Errorf("embed should be explicitly false: %+v", ev.Embed)
	}

	// A non-capabilities frame must report SupportsVision() == false
	// (fields absent, not false-by-default confusion).
	var ready inferd.AdminEvent
	if err := json.Unmarshal([]byte(`{"id":"admin","type":"status","status":"ready"}`), &ready); err != nil {
		t.Fatalf("decode ready frame: %v", err)
	}
	if ready.IsCapabilities() || ready.SupportsVision() {
		t.Errorf("ready frame misclassified: caps=%v vision=%v",
			ready.IsCapabilities(), ready.SupportsVision())
	}
}
