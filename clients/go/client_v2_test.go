package inferd_test

import (
	"bufio"
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net"
	"strings"
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
	// Use net.Pipe for an in-memory connection (no real socket).
	srvConn, clientConn := net.Pipe()
	defer srvConn.Close()
	defer clientConn.Close()

	srvDone := make(chan struct{})
	go func() {
		defer close(srvDone)
		br := bufio.NewReader(srvConn)

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
		srvWriteJSONFrame(srvConn, inferd.ResponseV2{
			ID: "v2-1", Type: inferd.ResponseV2Frame,
			Block: &inferd.ResponseBlockV2{Type: inferd.BlockText, Delta: "a red"},
		})
		srvWriteJSONFrame(srvConn, inferd.ResponseV2{
			ID: "v2-1", Type: inferd.ResponseV2Done,
			Usage:      &inferd.UsageV2{InputTokens: 276, OutputTokens: 2},
			StopReason: inferd.StopEndTurn,
			Backend:    "llamacpp",
		})
	}()

	// Construct a client from the client half of the pipe.
	client := inferd.New(clientConn)

	// 2x2 RGB: a tiny image. Consumer-decoded raw bytes per ADR 0016.
	rgb := []byte{
		220, 30, 30, 255, 255, 255,
		255, 255, 255, 220, 30, 30,
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
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

// TestCapabilitiesAudioSampleRate proves a consumer can read the sample
// rate audio attachments must use. Without this the daemon advertises a
// hard contract no Go caller can see, and a wrong rate is silently
// time-scaled by the encoder into a plausible wrong answer (#198/#199).
// The live frame from a Gemma 4 E4B mmproj is reproduced verbatim.
func TestCapabilitiesAudioSampleRate(t *testing.T) {
	line := []byte(`{"accelerator":"cpu","audio":true,"audio_sample_rate":16000,` +
		`"backend":"gemma-4-e4b","embed":false,"gpu_layers":0,"id":"admin",` +
		`"status":"capabilities","thinking":true,"tools":true,"type":"status",` +
		`"v2":true,"vision":true,"wire_version":1}`)
	var ev inferd.AdminEvent
	if err := json.Unmarshal(line, &ev); err != nil {
		t.Fatalf("decode capabilities frame: %v", err)
	}
	if !ev.SupportsAudio() {
		t.Errorf("SupportsAudio() = false, want true")
	}
	rate, ok := ev.RequiredAudioSampleRate()
	if !ok || rate != 16000 {
		t.Errorf("RequiredAudioSampleRate() = (%d, %v), want (16000, true)", rate, ok)
	}

	// An audio-capable backend that advertises no rate must report
	// absence, not a zero rate a caller would send verbatim.
	var noRate inferd.AdminEvent
	if err := json.Unmarshal([]byte(`{"id":"admin","type":"status",`+
		`"status":"capabilities","backend":"b","audio":true}`), &noRate); err != nil {
		t.Fatalf("decode rate-less frame: %v", err)
	}
	if _, ok := noRate.RequiredAudioSampleRate(); ok {
		t.Errorf("RequiredAudioSampleRate() reported a rate that was never advertised")
	}

	// An embed-only backend advertises audio:false and no rate.
	var embed inferd.AdminEvent
	if err := json.Unmarshal([]byte(`{"id":"admin","type":"status",`+
		`"status":"capabilities","backend":"embeddinggemma-300m","audio":false,`+
		`"embed":true}`), &embed); err != nil {
		t.Fatalf("decode embed frame: %v", err)
	}
	if embed.SupportsAudio() {
		t.Errorf("SupportsAudio() = true for an embed-only backend")
	}
	// Embed does not imply rerank — a bi-encoder has no classification
	// head, and rerank needs a RANK-pooling context it cannot share.
	if embed.SupportsRerank() {
		t.Errorf("SupportsRerank() = true for an embed-only backend")
	}
}

// TestRerankCapabilityIsIndependentOfEmbed pins the discovery contract for
// the ADR 0027 surface: rerank is advertised on its own key, absence means
// unsupported (covering both an omitted false and a daemon predating the
// field), and neither capability may be inferred from the other.
func TestRerankCapabilityIsIndependentOfEmbed(t *testing.T) {
	var rr inferd.AdminEvent
	if err := json.Unmarshal([]byte(`{"id":"admin","type":"status",`+
		`"status":"capabilities","backend":"bge-reranker-v2-m3","v2":false,`+
		`"embed":false,"rerank":true}`), &rr); err != nil {
		t.Fatalf("decode rerank frame: %v", err)
	}
	if !rr.SupportsRerank() {
		t.Errorf("SupportsRerank() = false for rerank:true frame: %+v", rr)
	}
	if rr.Embed == nil || *rr.Embed {
		t.Errorf("a cross-encoder must report embed:false, got %+v", rr.Embed)
	}

	var absent inferd.AdminEvent
	if err := json.Unmarshal([]byte(`{"id":"admin","type":"status",`+
		`"status":"capabilities","backend":"gemma-4-e4b"}`), &absent); err != nil {
		t.Fatalf("decode generation frame: %v", err)
	}
	if absent.SupportsRerank() {
		t.Errorf("SupportsRerank() = true with the key absent")
	}
	if absent.Rerank != nil {
		t.Errorf("absent rerank must stay nil, not default to false: %+v", absent.Rerank)
	}
}

// TestAudioAttachmentConstructor proves AudioAttachment produces the wire
// shape the daemon expects: kind=audio, the rate carried in JSON, and the
// f32 PCM bytes excluded from the JSON (they ride in a BLOB frame per
// ADR 0021).
func TestAudioAttachmentConstructor(t *testing.T) {
	pcm := []byte{0x00, 0x00, 0x80, 0x3f} // 1.0f32 LE
	att := inferd.AudioAttachment("a1", 16000, pcm)
	if att.Kind != inferd.AttachmentAudio || att.ID != "a1" || att.SampleRate != 16000 {
		t.Errorf("attachment fields unexpected: %+v", att)
	}
	if len(att.Bytes) != len(pcm) {
		t.Errorf("bytes not carried: %d", len(att.Bytes))
	}
	body, err := json.Marshal(att)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if got := string(body); !strings.Contains(got, `"sample_rate":16000`) {
		t.Errorf("sample_rate missing from JSON: %s", got)
	} else if strings.Contains(got, "Bytes") || strings.Contains(got, `"bytes"`) {
		t.Errorf("raw bytes leaked into the request JSON: %s", got)
	}
}

// The daemon parses tool_choice as a bare string, and rejects a value it
// does not recognise. An omitted choice must stay off the wire entirely:
// a spurious ""tool_choice":""" would be an unknown value, not an absent
// one, so the daemon would reject the request.
func TestToolChoiceWireShape(t *testing.T) {
	for _, c := range []inferd.ToolChoice{
		inferd.ToolChoiceAuto, inferd.ToolChoiceRequired, inferd.ToolChoiceNone,
	} {
		req := inferd.RequestV2{
			ID:         "tc",
			Messages:   []inferd.MessageV2{{Role: inferd.RoleUser, Content: []inferd.ContentBlock{inferd.TextBlock("hi")}}},
			Tools:      []inferd.ToolV2{{Name: "get_weather", Description: "w", InputSchema: json.RawMessage(`{"type":"object"}`)}},
			ToolChoice: c,
		}
		body, err := json.Marshal(req)
		if err != nil {
			t.Fatalf("marshal %s: %v", c, err)
		}
		want := `"tool_choice":"` + string(c) + `"`
		if !strings.Contains(string(body), want) {
			t.Errorf("want %s in %s", want, body)
		}
	}

	req := inferd.RequestV2{
		ID:       "tc",
		Messages: []inferd.MessageV2{{Role: inferd.RoleUser, Content: []inferd.ContentBlock{inferd.TextBlock("hi")}}},
	}
	body, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if strings.Contains(string(body), "tool_choice") {
		t.Errorf("absent tool_choice must not be serialised: %s", body)
	}
}

// TestToolChoiceUnsatisfiedWireShape pins the done-frame flag that
// reports "required was asked for and no call arrived" (ADR 0029).
//
// Two directions matter. Decoding a daemon's frame that sets it must
// surface true; decoding a frame that omits it — every frame a v0.7.0
// daemon ever sent — must yield false rather than failing the parse.
// Encoding must keep an unset flag off the wire so this stays additive.
func TestToolChoiceUnsatisfiedWireShape(t *testing.T) {
	set := `{"type":"done","id":"x","usage":{"input_tokens":9,"output_tokens":128},` +
		`"stop_reason":"max_tokens","backend":"llamacpp","tool_choice_unsatisfied":true}`
	var frame inferd.ResponseV2
	if err := json.Unmarshal([]byte(set), &frame); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if !frame.ToolChoiceUnsatisfied {
		t.Error("tool_choice_unsatisfied:true must decode as true")
	}
	// The stop reason stays max_tokens — the flag is the disambiguator,
	// not a replacement. A new StopReasonV2 value would not have
	// decoded at all against these fixed constants, which is why the
	// signal is a field.
	if frame.StopReason != inferd.StopMaxTokens {
		t.Errorf("stop_reason: got %q want max_tokens", frame.StopReason)
	}

	legacy := `{"type":"done","id":"x","usage":{"input_tokens":1,"output_tokens":1},` +
		`"stop_reason":"end_turn","backend":"llamacpp"}`
	frame = inferd.ResponseV2{}
	if err := json.Unmarshal([]byte(legacy), &frame); err != nil {
		t.Fatalf("legacy done frame must parse: %v", err)
	}
	if frame.ToolChoiceUnsatisfied {
		t.Error("absent field must default to false")
	}

	body, err := json.Marshal(inferd.ResponseV2{
		ID: "x", Type: inferd.ResponseV2Done,
		Usage:      &inferd.UsageV2{InputTokens: 1, OutputTokens: 1},
		StopReason: inferd.StopEndTurn,
		Backend:    "llamacpp",
	})
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if strings.Contains(string(body), "tool_choice_unsatisfied") {
		t.Errorf("unset flag must not be serialised: %s", body)
	}
}
