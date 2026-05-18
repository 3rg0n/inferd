package inferd

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestAdminEventDecodesDownloadFrame(t *testing.T) {
	raw := []byte(`{
		"id":"admin","type":"status","status":"loading_model","phase":"download",
		"downloaded_bytes":33554432,"total_bytes":5126304928,
		"source_url":"https://huggingface.co/example.gguf"
	}`)
	var ev AdminEvent
	if err := json.Unmarshal(raw, &ev); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if ev.ID != "admin" || ev.Type != "status" || ev.Status != "loading_model" {
		t.Errorf("envelope wrong: %+v", ev)
	}
	if ev.Phase != "download" || ev.DownloadedBytes != 33_554_432 {
		t.Errorf("phase/progress wrong: %+v", ev)
	}
	if ev.TotalBytes == nil || *ev.TotalBytes != 5_126_304_928 {
		t.Errorf("total_bytes wrong: %+v", ev)
	}
	if ev.SourceURL != "https://huggingface.co/example.gguf" {
		t.Errorf("source_url wrong: %+v", ev)
	}
}

func TestAdminEventDecodesReadyFrame(t *testing.T) {
	raw := []byte(`{"id":"admin","type":"status","status":"ready"}`)
	var ev AdminEvent
	if err := json.Unmarshal(raw, &ev); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if ev.Status != "ready" || ev.Phase != "" {
		t.Errorf("ready frame should have empty phase: %+v", ev)
	}
}

func TestAdminEventTotalBytesMayBeNull(t *testing.T) {
	// Spec: total_bytes can be `null` when Content-Length wasn't
	// supplied. Pointer field captures that.
	raw := []byte(`{"id":"admin","type":"status","status":"loading_model","phase":"download","downloaded_bytes":1024,"total_bytes":null,"source_url":"https://x"}`)
	var ev AdminEvent
	if err := json.Unmarshal(raw, &ev); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if ev.TotalBytes != nil {
		t.Errorf("total_bytes should be nil for null, got %v", *ev.TotalBytes)
	}
}

func TestIsTransientDialErrorRecognisesECONNREFUSED(t *testing.T) {
	cases := []string{
		"dial tcp 127.0.0.1:47321: connect: connection refused",
		"dial tcp 127.0.0.1:47321: connectex: No connection could be made because the target machine actively refused it",
		"open \\\\.\\pipe\\inferd-infer: The system cannot find the file specified.",
		"open /run/inferd/infer.sock: no such file or directory",
		"open \\\\.\\pipe\\inferd-infer: All pipe instances are busy.",
	}
	for _, msg := range cases {
		if !isTransientDialError(errors.New(msg)) {
			t.Errorf("expected transient: %q", msg)
		}
	}
}

func TestIsTransientDialErrorRejectsPermanent(t *testing.T) {
	cases := []string{
		"open /run/inferd/infer.sock: permission denied",
		"dial tcp: malformed address",
		"unknown nonsense error",
	}
	for _, msg := range cases {
		if isTransientDialError(errors.New(msg)) {
			t.Errorf("expected non-transient: %q", msg)
		}
	}
}

func TestDialAndWaitReadySucceedsImmediately(t *testing.T) {
	// Fixture: dialler that always succeeds on first call.
	calls := 0
	dial := func(ctx context.Context) (*Client, error) {
		calls++
		// Construct a fake Client wrapping a pipe pair; we don't
		// actually use it.
		c1, _ := net.Pipe()
		return New(c1), nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()
	client, err := DialAndWaitReady(ctx, dial)
	if err != nil {
		t.Fatalf("DialAndWaitReady: %v", err)
	}
	if calls != 1 {
		t.Errorf("expected 1 dial call, got %d", calls)
	}
	_ = client.Close()
}

func TestDialAndWaitReadyRetriesOnTransient(t *testing.T) {
	// Dialler that fails 2x with ECONNREFUSED then succeeds.
	calls := 0
	dial := func(ctx context.Context) (*Client, error) {
		calls++
		if calls < 3 {
			return nil, errors.New("dial tcp: connection refused")
		}
		c1, _ := net.Pipe()
		return New(c1), nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	client, err := DialAndWaitReady(ctx, dial)
	if err != nil {
		t.Fatalf("DialAndWaitReady: %v", err)
	}
	if calls != 3 {
		t.Errorf("expected 3 dial calls, got %d", calls)
	}
	_ = client.Close()
}

func TestDialAndWaitReadyDoesNotRetryOnPermanentError(t *testing.T) {
	calls := 0
	dial := func(ctx context.Context) (*Client, error) {
		calls++
		return nil, errors.New("permission denied")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()
	_, err := DialAndWaitReady(ctx, dial)
	if err == nil {
		t.Fatal("expected error")
	}
	if !strings.Contains(err.Error(), "permission denied") {
		t.Errorf("expected permission denied, got %v", err)
	}
	if calls != 1 {
		t.Errorf("expected 1 attempt for permanent error, got %d", calls)
	}
}

func TestDialAndWaitReadyHonoursContextCancel(t *testing.T) {
	dial := func(ctx context.Context) (*Client, error) {
		return nil, errors.New("connection refused")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()
	start := time.Now()
	_, err := DialAndWaitReady(ctx, dial)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("expected DeadlineExceeded, got %v", err)
	}
	if elapsed := time.Since(start); elapsed > 500*time.Millisecond {
		t.Errorf("retried for too long: %v", elapsed)
	}
}

func TestDefaultAdminAddrReturnsPlatformShape(t *testing.T) {
	got := DefaultAdminAddr()
	switch runtime.GOOS {
	case "linux":
		if got != "/run/inferd/admin.sock" {
			t.Errorf("linux: got %q", got)
		}
	case "windows":
		if got != `\\.\pipe\inferd-admin` {
			t.Errorf("windows: got %q", got)
		}
	case "darwin":
		if !strings.HasSuffix(got, "/inferd/admin.sock") {
			t.Errorf("darwin: got %q", got)
		}
	}
}
