package inferd_test

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	inferd "github.com/3rg0n/inferd/clients/go"
)

// TestProtocolRoundTripWithoutDaemon exercises the request/response
// shapes against a hand-rolled NDJSON peer — proves the Go types
// serialise to the wire format the Rust proto crate expects, without
// needing the daemon binary on PATH.
func TestProtocolRoundTripWithoutDaemon(t *testing.T) {
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

		// Read one Request.
		buf := make([]byte, 4096)
		n, _ := conn.Read(buf)
		var got inferd.Request
		if err := json.Unmarshal(trimNewline(buf[:n]), &got); err != nil {
			t.Errorf("decode request: %v", err)
			return
		}
		if got.ID != "rt-1" || len(got.Messages) != 1 {
			t.Errorf("unexpected request: %+v", got)
		}

		// Reply with one token frame and one done.
		writeFrame(conn, inferd.Response{
			ID: "rt-1", Type: inferd.ResponseToken, Content: "hi",
		})
		writeFrame(conn, inferd.Response{
			ID: "rt-1", Type: inferd.ResponseDone,
			Content:    "hi",
			StopReason: inferd.StopEnd,
			Backend:    "mock",
			Usage:      &inferd.Usage{PromptTokens: 1, CompletionTokens: 1},
		})
	}()

	addr := ln.Addr().String()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := inferd.DialTCP(ctx, addr)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer client.Close()

	stream, err := client.Generate(ctx, inferd.Request{
		ID: "rt-1",
		Messages: []inferd.Message{
			{Role: inferd.RoleUser, Content: "hello"},
		},
	})
	if err != nil {
		t.Fatalf("generate: %v", err)
	}

	var got []inferd.Response
	for f := range stream {
		got = append(got, f)
	}
	<-srvDone

	if len(got) != 2 {
		t.Fatalf("expected 2 frames, got %d: %+v", len(got), got)
	}
	if got[0].Type != inferd.ResponseToken || got[0].Content != "hi" {
		t.Errorf("frame[0] unexpected: %+v", got[0])
	}
	if got[1].Type != inferd.ResponseDone || got[1].Backend != "mock" ||
		got[1].StopReason != inferd.StopEnd {
		t.Errorf("frame[1] unexpected: %+v", got[1])
	}
}

// TestEndToEndAgainstDaemon launches the Rust daemon binary with the
// mock backend over TCP, sends one request, and verifies the response
// matches what the daemon's Mock emits. Skips when the binary isn't
// built (set INFERD_DAEMON_BIN to override the path).
func TestEndToEndAgainstDaemon(t *testing.T) {
	bin := os.Getenv("INFERD_DAEMON_BIN")
	if bin == "" {
		bin = defaultDaemonBin(t)
	}
	if _, err := os.Stat(bin); err != nil {
		t.Skipf("inferd-daemon binary not found at %s; "+
			"build it with `cargo build -p inferd-daemon` "+
			"or set INFERD_DAEMON_BIN.", bin)
	}

	tmp := t.TempDir()
	lock := filepath.Join(tmp, "inferd.lock")
	logDir := filepath.Join(tmp, "logs")
	adminSock := testAdminAddr(tmp)

	// Pick a free port for the daemon to bind by asking the OS for one
	// then immediately closing — small TOCTOU window but fine for a
	// local test.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("probe port: %v", err)
	}
	addr := ln.Addr().String()
	ln.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	cmd := exec.CommandContext(ctx, bin,
		"--backend", "mock",
		"--lock", lock,
		"--tcp", addr,
		"--admin-addr", adminSock,
	)
	cmd.Env = append(os.Environ(), "INFERD_LOG_DIR="+logDir)
	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatalf("stdout: %v", err)
	}
	stderrPipe, err := cmd.StderrPipe()
	if err != nil {
		t.Fatalf("stderr: %v", err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatalf("start daemon: %v", err)
	}
	defer func() {
		_ = stdoutPipe.Close()
		_ = stderrPipe.Close()
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	}()
	go drainPipe(stdoutPipe)
	go drainPipe(stderrPipe)

	// Wait for the daemon to bind. Retry the connect for up to 5s.
	var client *inferd.Client
	deadline := time.Now().Add(5 * time.Second)
	for {
		c, dialErr := inferd.DialTCP(ctx, addr)
		if dialErr == nil {
			client = c
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("daemon never bound %s: %v", addr, dialErr)
		}
		time.Sleep(50 * time.Millisecond)
	}
	defer client.Close()

	stream, err := client.Generate(ctx, inferd.Request{
		ID: "go-e2e-1",
		Messages: []inferd.Message{
			{Role: inferd.RoleUser, Content: "ping"},
		},
	})
	if err != nil {
		t.Fatalf("generate: %v", err)
	}

	var done *inferd.Response
	for f := range stream {
		if f.Type == inferd.ResponseDone {
			d := f
			done = &d
		}
	}
	if done == nil {
		t.Fatal("no Done frame received")
	}
	if done.ID != "go-e2e-1" {
		t.Errorf("done id: got %q want %q", done.ID, "go-e2e-1")
	}
	if done.Backend != "mock" {
		t.Errorf("done backend: got %q want mock", done.Backend)
	}
	if done.StopReason != inferd.StopEnd {
		t.Errorf("done stop_reason: got %q want end", done.StopReason)
	}
}

// testAdminAddr returns a per-test admin endpoint path that the daemon can
// bind without requiring root. On Windows it must be a named-pipe path.
func testAdminAddr(tmp string) string {
	if runtime.GOOS == "windows" {
		// Named pipe names are globally unique; embed the pid so parallel
		// test runs don't collide.
		return fmt.Sprintf(`\\.\pipe\inferd-test-admin-%d`, os.Getpid())
	}
	return filepath.Join(tmp, "admin.sock")
}

func defaultDaemonBin(t *testing.T) string {
	t.Helper()
	// Walk up from the test's working dir until we hit the workspace
	// root (a Cargo.toml is the giveaway), then look in target/debug.
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	for dir := cwd; dir != filepath.Dir(dir); dir = filepath.Dir(dir) {
		if _, err := os.Stat(filepath.Join(dir, "Cargo.toml")); err == nil {
			name := "inferd-daemon"
			if runtime.GOOS == "windows" {
				name += ".exe"
			}
			return filepath.Join(dir, "target", "debug", name)
		}
	}
	t.Fatalf("could not locate workspace root from %s", cwd)
	return ""
}

func writeFrame(w net.Conn, r inferd.Response) {
	b, _ := json.Marshal(r)
	b = append(b, '\n')
	_, _ = w.Write(b)
}

func trimNewline(b []byte) []byte {
	return []byte(strings.TrimRight(string(b), "\r\n"))
}

func drainPipe(rc interface {
	Read(p []byte) (int, error)
}) {
	buf := make([]byte, 4096)
	for {
		_, err := rc.Read(buf)
		if err != nil {
			if !errors.Is(err, os.ErrClosed) {
				_ = fmt.Errorf("drain: %w", err)
			}
			return
		}
	}
}
