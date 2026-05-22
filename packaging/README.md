# inferd packaging

Service-install manifests for the inferd-daemon binary, one per
platform. Each is hardened per `THREAT_MODEL.md` F-16; the trade-offs
that lead to the specific directives are documented inline in the
manifests themselves rather than duplicated here.

## Layout

- `systemd/inferd.service` — Linux per-user systemd unit. Install at
  `~/.config/systemd/user/inferd.service`.
- `launchd/io.inferd.daemon.plist` — macOS LaunchAgent template.
  Use `packaging/launchd/install-launchagent.sh [/path/to/binary]`
  to install — the script substitutes per-user paths (HOME and TMPDIR)
  that launchd does not expand in plist values, then bootstraps the
  agent. `packaging/launchd/uninstall-launchagent.sh` reverses it.
- `windows/install.ps1` — Windows per-user installer. Adds a
  Startup-folder shortcut to `%LOCALAPPDATA%\inferd\inferd-daemon.exe`
  so the daemon runs as the logged-in user on every login. **No
  elevation required.** Pair with `windows/uninstall.ps1` to remove.

The release workflow (`.github/workflows/release.yml`) bundles each
manifest into the matching platform's archive (M4 packaging
follow-up, tracked separately from the alpha tag).

## What hardening is and isn't applied

| Layer | Linux (systemd --user) | macOS (launchd LaunchAgent) | Windows (Startup shortcut) |
|---|---|---|---|
| Privilege drop | `CapabilityBoundingSet=` (empty) | LaunchAgent (per-user) | per-user (logged-in user only) |
| Filesystem isolation | `ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp=yes` | macOS app sandbox when signed | none (per-user profile only) |
| Service-control ACL | kernel-enforced unit ownership | LaunchAgent per-user | n/a — no SCM service registered |
| Syscall filter | `SystemCallFilter=@system-service` | n/a | n/a |
| Restart on crash | `Restart=on-failure` | `KeepAlive` + `ThrottleInterval` | re-launched on next login |
| Memory write+exec | `MemoryDenyWriteExecute=yes` | n/a | n/a |
| Namespace isolation | `RestrictNamespaces=yes` | n/a | n/a |

All three install paths are per-user, no-elevation. The Windows
posture is structurally simpler than the v0.2.1 SCM-service shape:
no `sc.exe`, no NetworkService, no SDDL hardening. The daemon binds
named pipes with the standard creator-owned DACL. A second user on
the same machine cannot displace the running daemon because they
cannot terminate processes in another user's session without admin
rights. If two users on the same box both install inferd, they each
get their own daemon and their own pipes — same isolation model as
the macOS LaunchAgent.

## After install

The activity log lives at the platform-default path
(`~/.inferd/logs/inferd.ndjson` or wherever `INFERD_LOG_DIR` points).
Tail it during initial bring-up to confirm the daemon binds the
expected listener and reports `connection_accepted` events for each
client connect.
