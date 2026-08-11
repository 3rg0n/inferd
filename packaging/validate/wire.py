#!/usr/bin/env python3
"""Raw wire client for install=work validation. Stdlib only.

Speaks both live surfaces directly rather than through `inferd-client`,
so the framing itself is exercised:

- generation: `[uvarint payload_len][1 byte type: 0x01 JSON / 0x02 BLOB]
  [payload]`, where `payload_len` counts the payload only
  (`crates/inferd-proto/src/frame.rs`)
- embeddings: NDJSON, one request line, one terminal frame

Transport is chosen from the platform: AF_UNIX on Unix, named pipes via
ctypes on Windows. Socket paths follow the platform defaults and can be
overridden with INFERD_SOCK / INFERD_EMBED_SOCK; the client-side read
timeouts with INFERD_GEN_TIMEOUT / INFERD_EMBED_TIMEOUT.
"""
import json
import os
import sys

# The "v2" in the generation surface's name is the *surface* generation,
# not the wire version: `inferd_proto::v2::WIRE_VERSION` is 1 and has
# never moved (ADR 0021). Sending 2 earns a correct
# `wire_version_unsupported`, which looks alarmingly like a real defect.
WIRE_VERSION = 1

IS_WINDOWS = sys.platform == "win32"


def _default_paths():
    """Where the *installed* daemon binds, mirroring the daemon itself.

    Tracks `inferd-daemon/src/endpoint.rs`:

    - Linux: `$XDG_RUNTIME_DIR/inferd/`, else `~/.inferd/run/`. The
      home fallback is Linux-only.
    - macOS: `std::env::temp_dir()/inferd/`, i.e. `$TMPDIR/inferd/` --
      macOS rotates the per-user temp dir per login, and the launchd
      plist substitutes the same path at install time. **Not**
      `~/.inferd/run`.
    - Windows: named pipes.
    """
    if IS_WINDOWS:
        return r"\\.\pipe\inferd", r"\\.\pipe\inferd-infer-embed"
    if sys.platform == "darwin":
        import tempfile  # honours TMPDIR, as std::env::temp_dir() does

        d = os.path.join(tempfile.gettempdir(), "inferd")
    else:
        run = os.environ.get("XDG_RUNTIME_DIR")
        d = (os.path.join(run, "inferd") if run
             else os.path.expanduser("~/.inferd/run"))
    return os.path.join(d, "inferd.sock"), os.path.join(d, "infer.embed.sock")


_GEN_DEFAULT, _EMB_DEFAULT = _default_paths()
GEN = os.environ.get("INFERD_SOCK", _GEN_DEFAULT)
EMB = os.environ.get("INFERD_EMBED_SOCK", _EMB_DEFAULT)

# Client-side read timeouts, in seconds. These bound how long the *client*
# waits, nothing on the wire, so widening one cannot mask a daemon defect
# -- but too tight a value reads exactly like a hang. The defaults suit an
# accelerated host; a slow decode is not a failure:
#
# - CPU-only targets (the arm64 legs) decode far slower than CUDA/Metal.
# - Metal JIT-compiles each new kernel-shape variant on first use, and on
#   a memory-pressured box the v0.8.0 macOS leg saw one adversarial-prompt
#   generation exceed 180s that way while `doctor` stayed `ready`
#   throughout and the result was byte-for-byte correct.
#
# Raise these rather than editing this file or wrapping it -- that leg had
# to build a throwaway wrapper, which is the scratch-rebuild this
# committed harness exists to stop.
GEN_TIMEOUT = int(os.environ.get("INFERD_GEN_TIMEOUT", "180"))
EMB_TIMEOUT = int(os.environ.get("INFERD_EMBED_TIMEOUT", "120"))


def uvarint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


# --- transport ------------------------------------------------------------
# Both implementations expose the same duck type: send(bytes),
# read(n) -> bytes (short reads looped away), close().

class _UnixConn:
    def __init__(self, path, timeout):
        import socket

        self._s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._s.settimeout(timeout)
        self._s.connect(path)
        self._f = self._s.makefile("rb")

    def send(self, data):
        self._s.sendall(data)

    def read(self, n):
        return self._f.read(n)

    def close(self):
        self._s.close()


class _PipeConn:
    """Windows named pipe via kernel32. Mirrors the Windows-leg client."""

    def __init__(self, path, timeout):
        import ctypes
        from ctypes import wintypes

        self._k = ctypes.WinDLL("kernel32", use_last_error=True)
        GENERIC_RW = 0xC0000000
        OPEN_EXISTING = 3
        self._k.CreateFileW.restype = wintypes.HANDLE
        h = self._k.CreateFileW(
            path, GENERIC_RW, 0, None, OPEN_EXISTING, 0, None
        )
        if h == wintypes.HANDLE(-1).value:
            raise OSError(ctypes.get_last_error(), f"CreateFileW {path}")
        self._h = h
        self._ct = ctypes

    def send(self, data):
        written = self._ct.wintypes.DWORD(0)
        if not self._k.WriteFile(
            self._h, data, len(data), self._ct.byref(written), None
        ):
            raise OSError(self._ct.get_last_error(), "WriteFile")

    def read(self, n):
        buf = self._ct.create_string_buffer(n)
        got = self._ct.wintypes.DWORD(0)
        out = b""
        while len(out) < n:
            if not self._k.ReadFile(
                self._h, buf, n - len(out), self._ct.byref(got), None
            ):
                break
            if got.value == 0:
                break
            out += buf.raw[: got.value]
        return out

    def close(self):
        self._k.CloseHandle(self._h)


def _connect(path, timeout):
    return _PipeConn(path, timeout) if IS_WINDOWS else _UnixConn(path, timeout)


def _read_uvarint(conn):
    shift = 0
    val = 0
    while True:
        b = conn.read(1)
        if not b:
            return None
        val |= (b[0] & 0x7F) << shift
        if not b[0] & 0x80:
            return val
        shift += 7


# --- surfaces -------------------------------------------------------------

def gen(req, timeout=None):
    """Send one RequestV2; return every response frame."""
    req.setdefault("wire_version", WIRE_VERSION)
    conn = _connect(GEN, GEN_TIMEOUT if timeout is None else timeout)
    payload = json.dumps(req).encode()
    conn.send(uvarint(len(payload)) + b"\x01" + payload)
    frames = []
    while True:
        n = _read_uvarint(conn)
        if n is None:
            break
        kind = conn.read(1)
        body = conn.read(n)
        if kind == b"\x01":
            frames.append(json.loads(body))
            if frames[-1].get("type") in ("done", "error"):
                break
        else:
            frames.append({"type": "blob", "len": len(body)})
    conn.close()
    return frames


def embed(req, timeout=None):
    """Send one embed request (NDJSON); return the terminal frame."""
    conn = _connect(EMB, EMB_TIMEOUT if timeout is None else timeout)
    conn.send(json.dumps(req).encode() + b"\n")
    line = b""
    while not line.endswith(b"\n"):
        chunk = conn.read(1)
        if not chunk:
            break
        line += chunk
    conn.close()
    return json.loads(line)


# --- frame extraction -----------------------------------------------------
# Streaming frames are {"type":"frame","block":{...}} -- the typed content
# block carries the payload. Text arrives as incremental `delta`s; a
# tool_use arrives as one complete block. Reading these as
# {"type":"token","text":...} yields empty text on every gate while the
# terminal frames look perfect: a green terminal frame is not a green gate.

def _blocks(frames, kind):
    for fr in frames:
        if fr.get("type") != "frame":
            continue
        blk = fr.get("block")
        if isinstance(blk, dict) and blk.get("type") == kind:
            yield blk


def text_of(frames):
    return "".join(b.get("delta", "") for b in _blocks(frames, "text"))


def tool_uses(frames):
    return list(_blocks(frames, "tool_use"))


def terminal(frames):
    return next((fr for fr in frames if fr.get("type") in ("done", "error")), None)


if __name__ == "__main__":
    print(f"generation: {GEN}\nembeddings: {EMB}")
    print(f"timeouts:   gen={GEN_TIMEOUT}s embed={EMB_TIMEOUT}s")
    if len(sys.argv) > 1:
        print(json.dumps(gen(json.loads(sys.argv[1])), indent=1))
