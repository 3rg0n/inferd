#!/usr/bin/env bash
# install-launchagent.sh — install the inferd-daemon LaunchAgent for the current user.
#
# Usage:
#   ./packaging/launchd/install-launchagent.sh [/path/to/inferd-daemon]
#
# If the binary path is omitted, /usr/local/bin/inferd-daemon is used.
# Run from the repository root or any subdirectory — the script locates
# the plist template relative to itself.

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

mkdir -p "$AGENTS_DIR"
mkdir -p "$LOG_DIR"

# Substitute placeholders:  __HOME__, __TMPDIR__, __BIN__.
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
echo "Status:"
launchctl list "$LABEL" 2>/dev/null || echo "(list not available)"
