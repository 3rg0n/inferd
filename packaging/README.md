# inferd packaging

Service-install manifests for the inferd-daemon binary, one per
platform. Each is hardened per `THREAT_MODEL.md` F-16; the trade-offs
that lead to the specific directives are documented inline in the
manifests themselves rather than duplicated here.

## Layout

- `systemd/inferd.service` — Linux per-user systemd unit. Install at
  `~/.config/systemd/user/inferd.service`.
- `launchd/io.inferd.daemon.plist` — macOS LaunchAgent. Install at
  `~/Library/LaunchAgents/io.inferd.daemon.plist`. Edit the
  `USERNAME_HERE` placeholders before installing — launchd doesn't
  expand `$HOME` in plist paths.
- `windows/install.ps1` — Windows service installer. Run elevated.

The release workflow (`.github/workflows/release.yml`) bundles each
manifest into the matching platform's archive (M4 packaging
follow-up, tracked separately from the alpha tag).

## What hardening is and isn't applied

| Layer | Linux (systemd) | macOS (launchd) | Windows (sc.exe) |
|---|---|---|---|
| Privilege drop | `CapabilityBoundingSet=` (empty) | LaunchAgent (per-user) | `obj= NT AUTHORITY\NetworkService` |
| Filesystem isolation | `ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp=yes` | macOS app sandbox when signed | none |
| Syscall filter | `SystemCallFilter=@system-service` | n/a | n/a |
| Restart on crash | `Restart=on-failure` | `KeepAlive` + `ThrottleInterval` | `sc.exe failure restart/2000` |
| Memory write+exec | `MemoryDenyWriteExecute=yes` | n/a | n/a |
| Namespace isolation | `RestrictNamespaces=yes` | n/a | n/a |

The Windows posture is the weakest of the three. Service-ACL hardening
(`sc.exe sdshow` / `sdset` to deny non-admins from controlling the
service) is post-alpha work tracked under THREAT_MODEL F-16.

## After install

The activity log lives at the platform-default path
(`~/.inferd/logs/inferd.ndjson` or wherever `INFERD_LOG_DIR` points).
Tail it during initial bring-up to confirm the daemon binds the
expected listener and reports `connection_accepted` events for each
client connect.
