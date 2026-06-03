#!/usr/bin/env bash
# uninstall-launchagent.sh — remove the inferd-daemon LaunchAgent for the current user.
#
# Usage:  ./packaging/launchd/uninstall-launchagent.sh

set -euo pipefail

DEST="$HOME/Library/LaunchAgents/io.inferd.daemon.plist"
UID_VAL="$(id -u)"
LABEL="io.inferd.daemon"

if launchctl list "$LABEL" &>/dev/null; then
    launchctl bootout "gui/$UID_VAL" "$DEST" 2>/dev/null || true
    echo "Agent stopped and booted out."
else
    echo "Agent not currently loaded — nothing to stop."
fi

launchctl disable "gui/$UID_VAL/$LABEL" 2>/dev/null || true

if [[ -f "$DEST" ]]; then
    rm "$DEST"
    echo "Removed $DEST"
else
    echo "$DEST already absent."
fi

# ADR 0019 / phase 5d: the install script flattened `backends/*.dylib`
# next to the daemon binary. We don't remove them here because we
# don't know where the operator put the daemon — the plist isn't read
# back, and removing the wrong libllama.dylib could break unrelated
# software. Operators wanting to fully clean up a release-tarball
# install can rm -rf the extraction dir; the plist is the only thing
# this script tracks.

echo "Uninstall complete."
