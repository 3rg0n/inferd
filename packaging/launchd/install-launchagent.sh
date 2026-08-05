#!/usr/bin/env bash
# install-launchagent.sh — install the inferd-daemon LaunchAgent for the current user.
#
# Usage:
#   ./packaging/launchd/install-launchagent.sh [/path/to/inferd-daemon]
#
# If the binary path is omitted, /usr/local/bin/inferd-daemon is used.
# Run from the repository root or any subdirectory — the script locates
# the plist template relative to itself.
#
# The daemon reads ~/.inferd/config.json on startup. On first boot the
# daemon writes a pinned multi-backend default config (real llamacpp
# generate + embed, both with auto_pull = true) — no `inferd pull`
# precondition, no `--backend` / `--model-path` argument substitution
# required from this script.
#
# In the airgapped archive (ADR 0028) that config is written the same way
# but cannot be satisfied by fetching; the closing message detects that
# from the binary itself and points at `inferdctl import` instead.

set -euo pipefail

BIN="${1:-/usr/local/bin/inferd-daemon}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="$SCRIPT_DIR/io.inferd.daemon.plist"
DEST="$HOME/Library/LaunchAgents/io.inferd.daemon.plist"
AGENTS_DIR="$HOME/Library/LaunchAgents"
LOG_DIR="$HOME/Library/Logs/inferd"

# TMPDIR on macOS is a per-user, per-session path provisioned by launchd
# (e.g. /var/folders/.../T/). `getconf DARWIN_USER_TEMP_DIR` gives the
# stable value that matches what the daemon's default_admin_addr() returns.
TMPDIR_REAL="$(getconf DARWIN_USER_TEMP_DIR)"
# Ensure it ends with exactly one slash so __TMPDIR__inferd becomes
# /var/folders/.../T/inferd (no double slash, no missing slash).
TMPDIR_REAL="${TMPDIR_REAL%/}/"

if [[ ! -f "$BIN" ]]; then
    echo "error: binary not found at $BIN" >&2
    echo "       Build it first:  cargo build --release -p inferd-daemon" >&2
    echo "       Then re-run:     $0 target/release/inferd-daemon" >&2
    exit 1
fi
# Resolve to an absolute path so launchd's plist never contains a
# relative path (launchd resolves Program relative to /, not the
# caller's cwd, which causes EX_NOINPUT / exit-78 at launch time).
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

# ADR 0019 / phase 5d: ggml_backend_load_all() searches the
# executable's own directory + cwd for ggml-* MODULE libs; subdirs
# are NOT searched. The release tarball ships them in `backends/`
# as a subdir; for the daemon to load them at runtime they must live
# next to the daemon binary, not under `backends/`.
#
# Three layouts to support:
#   1. Release tarball, freshly extracted to e.g. ~/inferd-v0.3.0/.
#      Contains ./inferd-daemon and ./backends/libllama.dylib +
#      ./backends/libggml-*.dylib. Flatten `backends/*` into the
#      parent.
#   2. Same dir as (1) but already flattened from a prior install.
#      libllama.dylib already next to the daemon. No-op.
#   3. Cargo build: $BIN at e.g. ./target/release/inferd-daemon and
#      libs at ./target/release/backends/. Flatten as in (1).
#
# We refuse to write into directories we don't own (e.g. user passed
# /usr/local/bin/inferd-daemon). The release-tarball common case is
# safe — extraction puts everything in one user-owned dir.
BIN_DIR="$(cd "$(dirname "$BIN")" && pwd)"
if [[ -d "$BIN_DIR/backends" ]]; then
    if [[ -w "$BIN_DIR" ]]; then
        echo "Flattening $BIN_DIR/backends/ -> $BIN_DIR/"
        # Copy both .dylib (shared libs: libllama, libggml, libggml-base) and
        # .so (backend MODULE libs: libggml-metal.so, libggml-cpu.so, libggml-blas.so).
        # ggml_backend_load_all uses .so extension on all Unix platforms including macOS.
        # Run as a subshell with nullglob so missing globs don't abort the script.
        (shopt -s nullglob; cp -f "$BIN_DIR/backends/"*.dylib "$BIN_DIR/backends/"*.so "$BIN_DIR/" 2>/dev/null || true)
    else
        echo "error: $BIN_DIR/backends/ exists but $BIN_DIR is not writable" >&2
        echo "       Move both inferd-daemon and the contents of backends/ into a user-owned dir," >&2
        echo "       e.g. ~/.local/bin/, then re-run this script with the new path." >&2
        exit 1
    fi
fi
if [[ ! -f "$BIN_DIR/libllama.dylib" ]]; then
    echo "error: $BIN_DIR/libllama.dylib missing — daemon would fail at startup." >&2
    echo "       Expected layout: inferd-daemon AND libllama.dylib AND libggml*.dylib siblings in one dir." >&2
    exit 1
fi

mkdir -p "$AGENTS_DIR"
mkdir -p "$LOG_DIR"

# Substitute placeholders: __HOME__, __TMPDIR__, __BIN__.
sed \
    -e "s|__HOME__|${HOME}|g" \
    -e "s|__TMPDIR__|${TMPDIR_REAL}|g" \
    -e "s|__BIN__|${BIN}|g" \
    "$TEMPLATE" > "$DEST"

echo "Installed plist → $DEST"

UID_VAL="$(id -u)"
LABEL="io.inferd.daemon"

# If the agent is already loaded, unload it first so edits take effect.
if launchctl list "$LABEL" &>/dev/null; then
    echo "Agent already loaded — stopping and re-bootstrapping..."
    launchctl bootout "gui/$UID_VAL" "$DEST" 2>/dev/null || true
fi

# enable must come before bootstrap: if the agent was previously
# bootout-ed, launchd marks it disabled and bootstrap returns EX_IO (5)
# unless enable has been called first to clear the disabled flag.
launchctl enable "gui/$UID_VAL/$LABEL"
launchctl bootstrap "gui/$UID_VAL" "$DEST"

echo "Agent bootstrapped and enabled."
echo
echo "Sockets and lock live under: ${TMPDIR_REAL}inferd/"
echo "Logs: $LOG_DIR/"
echo
echo "On first boot the daemon will write ~/.inferd/config.json (if absent)."
# One installer ships in both release archives (ADR 0028), and only one of
# them can fetch models. Ask the binary rather than guessing: it prints its
# own build profile, so this message cannot drift from what got installed.
# `--version` just prints and exits — it takes no single-instance lock.
#
# Three outcomes, not two: if `--version` can't be read, say so rather
# than printing the networked message on a guess. Guessing "networked"
# on an airgapped install tells the operator to wait for a pull that
# will never start, which is the worst of the three to get wrong.
PROFILE_TEXT="$("$BIN" --version 2>&1 || true)"
case "$PROFILE_TEXT" in
    *"build profile: airgapped"*)
        echo "This is an AIRGAPPED build: no HTTPS client is linked, so it will not"
        echo "fetch models. Import them from local files, then clear each source_url"
        echo "in config.json:"
        echo "  inferdctl import --name gemma-4-e4b <path.gguf>"
        echo "See airgapped.md in the archive root for the full runbook."
        ;;
    *"build profile: networked"*)
        echo "It then pulls the configured generate + embed models into the CAS"
        echo "store. Watch progress with:  inferdctl watch"
        ;;
    *)
        echo "Could not read the build profile from '$BIN --version', so this"
        echo "script can't tell whether it fetches models. Run:"
        echo "  inferd-daemon --version"
        echo "A 'networked' build pulls models on first boot (inferdctl watch); an"
        echo "'airgapped' build needs inferdctl import (see airgapped.md in the"
        echo "archive root)."
        ;;
esac
echo
echo "Status:"
launchctl list "$LABEL" 2>/dev/null || echo "(list not available)"
