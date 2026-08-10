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

`validate/` in the repo (not bundled into archives) holds the
stdlib-only Python gates each platform's **install=work** leg runs
against the installed daemon — see `packaging/validate/README.md`.

## ADR 0019: backends/ co-location

v0.3 builds use the dynamic-loader path: `libllama` is a shared
library and every ggml backend (`libggml`, `libggml-base`,
`ggml-cpu-*` CPU variants, plus accelerator MODULEs like
`ggml-metal` / `ggml-cuda` / `ggml-vulkan` / `ggml-hip`) is a MODULE
library that libllama dlopen's at runtime.

Release tarballs ship those libs in a `backends/` subdir alongside
the daemon. **At install time the libs must live next to the daemon
binary**, not under a `backends/` subdir — `ggml_backend_load_all()`
searches only the executable's own directory and cwd. The included
install scripts handle this:

| Script | Behavior |
|---|---|
| `windows/install.ps1` | When `-SourceBinary` is given, copies `<source>\backends\*.dll` into `%LOCALAPPDATA%\inferd\` next to `inferd-daemon.exe`. Operators staging by hand should do the same. |
| `launchd/install-launchagent.sh` | Detects a `backends/` sibling of the binary; if `<bindir>/libllama.dylib` is missing, flattens `backends/*.dylib` into `<bindir>/`. Refuses to write to dirs the user doesn't own. |
| `systemd/inferd.service` | The unit expects `~/.local/bin/inferd-daemon` plus `libllama.so` + `libggml-*.so` siblings in `~/.local/bin/`. From the release tarball: `cp inferd-v*-linux-*/inferd-daemon ~/.local/bin/ && cp inferd-v*-linux-*/backends/* ~/.local/bin/`. |

The daemon binary itself has `RPATH=$ORIGIN` (Linux) /
`@loader_path` (macOS) baked in at link time, so libllama+ggml-*
load from `<bindir>/` without `LD_LIBRARY_PATH` or
`DYLD_LIBRARY_PATH` gymnastics. Windows uses the OS loader's
exe-dir-first search, no equivalent flag needed.

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
expected listener and reports a per-surface accept event for each
client connect — `v2_connection_accepted` on the generation socket,
`embed_connection_accepted` on the embeddings socket.
