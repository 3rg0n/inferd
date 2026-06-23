package inferd_test

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
	"time"

	inferd "github.com/3rg0n/inferd/clients/go"
)

// TestEndToEndAgainstDaemon launches the Rust daemon binary with the
// mock backend over the default transport (named pipe on Windows, UDS on Unix),
// sends one request, and verifies the response matches what the daemon's Mock
// emits. Skips when the binary isn't built (set INFERD_DAEMON_BIN to override
// the path).
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
	inferSock := testInferAddr(tmp)

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	cmd := exec.CommandContext(ctx, bin,
		"--backend", "mock",
		"--lock", lock,
		testInferFlag(),
		inferSock,
		"--admin-addr", adminSock,
	)
	cmd.Env = append(os.Environ(),
		"INFERD_LOG_DIR="+logDir,
		// Isolate the test daemon from the real ~/.inferd/config.json so it
		// doesn't attempt to load a real llamacpp model instead of mock.
		"HOME="+tmp,
		"USERPROFILE="+tmp,
	)
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
		c, dialErr := inferd.DialInfer(ctx)
		if dialErr == nil {
			client = c
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("daemon never bound: %v", dialErr)
		}
		time.Sleep(50 * time.Millisecond)
	}
	defer client.Close()

	// v0.4 (ADR 0021): the daemon serves the v2 length-prefixed framing
	// on its single generation socket, so the e2e round-trip uses
	// GenerateV2. This exercises the real client<->daemon wire (frame
	// codec + wire_version) against the mock backend, no GPU needed.
	stream, err := client.GenerateV2(ctx, inferd.RequestV2{
		ID: "go-e2e-1",
		Messages: []inferd.MessageV2{
			{Role: inferd.RoleUser, Content: []inferd.ContentBlock{inferd.TextBlock("ping")}},
		},
	})
	if err != nil {
		t.Fatalf("generate_v2: %v", err)
	}

	var done *inferd.ResponseV2
	for f := range stream {
		if f.Type == inferd.ResponseV2Error {
			t.Fatalf("error frame: code=%s msg=%s", f.Code, f.Message)
		}
		if f.Type == inferd.ResponseV2Done {
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
	if done.StopReason != inferd.StopEndTurn {
		t.Errorf("done stop_reason: got %q want end_turn", done.StopReason)
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

// testInferFlag returns the CLI flag for the test daemon's inference socket
// (--uds on Unix, --pipe on Windows).
func testInferFlag() string {
	if runtime.GOOS == "windows" {
		return "--pipe"
	}
	return "--uds"
}

// testInferAddr returns a per-test inference endpoint path that the daemon can
// bind without requiring root. On Windows it must be a named-pipe path.
func testInferAddr(tmp string) string {
	if runtime.GOOS == "windows" {
		// Named pipe names are globally unique; embed the pid so parallel
		// test runs don't collide.
		return fmt.Sprintf(`\\.\pipe\inferd-test-infer-%d`, os.Getpid())
	}
	return filepath.Join(tmp, "inferd.sock")
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
			// Pick the more recently built of target/release and
			// target/debug. A hardcoded release-first (or debug-first)
			// preference breaks whenever the *other* profile holds a
			// stale binary from an older version (e.g. a v0.3 release
			// build left over next to a fresh v0.4 debug build, or vice
			// versa) — that stale daemon speaks the wrong wire and the
			// LP round-trip fails. Newest-wins is correct on every box.
			release := filepath.Join(dir, "target", "release", name)
			debug := filepath.Join(dir, "target", "debug", name)
			rInfo, rErr := os.Stat(release)
			dInfo, dErr := os.Stat(debug)
			switch {
			case rErr == nil && dErr == nil:
				if rInfo.ModTime().After(dInfo.ModTime()) {
					return release
				}
				return debug
			case rErr == nil:
				return release
			default:
				// debug present or neither (caller's os.Stat skips cleanly).
				return debug
			}
		}
	}
	t.Fatalf("could not locate workspace root from %s", cwd)
	return ""
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
