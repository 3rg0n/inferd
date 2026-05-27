# inferd-daemon Windows per-user installer.
#
# Run as the current user — no elevation required:
#   powershell -ExecutionPolicy Bypass -File install.ps1
#
# Installs the daemon as a per-user Startup-folder shortcut. On login,
# Windows launches the .lnk, which runs the daemon as the current user
# (no SCM, no NetworkService, no service ACL). This matches the macOS
# LaunchAgent and Linux systemd --user posture: per-user, no elevation,
# stops when the user logs out.
#
# Architecture:
#   - Daemon binary staged at      %LOCALAPPDATA%\inferd\inferd-daemon.exe
#   - Startup shortcut at          shell:startup\inferd-daemon.lnk
#     (i.e. %APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\)
#   - Logs at                      %LOCALAPPDATA%\inferd\logs\
#   - Lock at                      %LOCALAPPDATA%\inferd\inferd.lock
#   - Pipes at                     \\.\pipe\inferd-infer
#                                  \\.\pipe\inferd-infer-embed
#                                  (admin pipe uses the daemon default)
#
# After install the daemon is launched immediately so the operator does
# not need to log out / log in. The Startup shortcut handles the next
# boot.
#
# THREAT_MODEL F-16: per-user user-account isolation. The daemon binds
# named pipes with the default DACL (creator + Authenticated Users)
# matching the lifecycle's pipe-DACL helper. No service ACL needed
# because there is no service.

[CmdletBinding()]
param(
    [string]$BinaryPath    = "$env:LOCALAPPDATA\inferd\inferd-daemon.exe",
    [string]$LockPath      = "$env:LOCALAPPDATA\inferd\inferd.lock",
    [string]$PipePath      = "\\.\pipe\inferd-infer",
    [string]$EmbedPipePath = "\\.\pipe\inferd-infer-embed",
    [string]$LogDir        = "$env:LOCALAPPDATA\inferd\logs",
    [string]$ShortcutName  = "inferd-daemon.lnk",
    [string]$SourceBinary  = "",
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"

# Stage the binary into %LOCALAPPDATA%\inferd if -SourceBinary was given.
# Without it the script assumes the operator has already copied the
# binary to $BinaryPath (e.g. by extracting the release tarball there).
$installDir = Split-Path -Parent $BinaryPath
if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}
if ($SourceBinary -ne "") {
    if (-not (Test-Path $SourceBinary)) {
        Write-Error "source binary not found at $SourceBinary"
    }
    Write-Host "Staging $SourceBinary -> $BinaryPath"
    Copy-Item -Path $SourceBinary -Destination $BinaryPath -Force

    # ADR 0019 / phase 5d: stage the `backends/` subdir alongside the
    # binary. The release tarball ships
    #   inferd-<tag>-<target>/inferd-daemon.exe
    #   inferd-<tag>-<target>/backends/{ggml,ggml-base,llama,...}.dll
    # so the source-tree shape mirrors the install-tree shape. On
    # Windows the OS loader looks in the EXE's directory first, so
    # placing the DLLs in $installDir (next to the EXE) is enough — no
    # RPATH needed. Operators who skip -SourceBinary must copy
    # `backends\` themselves (the message below tells them how).
    $sourceDir     = Split-Path -Parent $SourceBinary
    $sourceBackend = Join-Path $sourceDir "backends"
    if (Test-Path $sourceBackend) {
        Write-Host "Staging $sourceBackend\* -> $installDir"
        Copy-Item -Path (Join-Path $sourceBackend "*") -Destination $installDir -Force
    } else {
        Write-Warning "no backends\ subdir at $sourceBackend; daemon will fail at runtime if libllama.dll isn't already next to the binary"
    }
}
if (-not (Test-Path $BinaryPath)) {
    Write-Error @"
inferd-daemon binary not found at $BinaryPath.
Either copy it there first, or pass -SourceBinary <path> to stage it:
    .\install.ps1 -SourceBinary .\target\release\inferd-daemon.exe

If staging by hand, copy the entire release tarball contents into
$installDir — including the backends\ subdir's DLLs (libllama,
libggml, libggml-base, ggml-cpu-*). The daemon dlopen's them from
the EXE's directory.
"@
}

if (-not (Test-Path $LogDir)) {
    New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
}

# User-level env vars. Setting them at User scope persists across logins
# so the Startup-launched daemon picks them up. Process-scope assignment
# also affects the immediate -StartNow launch below.
[System.Environment]::SetEnvironmentVariable("INFERD_LOG",     "info",  "User")
[System.Environment]::SetEnvironmentVariable("INFERD_LOG_DIR", $LogDir, "User")
$env:INFERD_LOG     = "info"
$env:INFERD_LOG_DIR = $LogDir

# If a previous install left a service behind, surface it so the
# operator can clean it up. The legacy installer registered an
# SCM service named "inferd-daemon"; if it's still present (running
# or zombie), the named-pipe paths collide with what the new
# Startup-launched daemon wants to bind.
& sc.exe query inferd-daemon 2>&1 | Out-Null
$legacyServicePresent = ($LASTEXITCODE -eq 0)
# `sc.exe query` returns 1060 when the service doesn't exist. Without
# this clear, the script inherits 1060 as its own exit (1060 mod 256
# = 36), which packaging tooling reads as a failed install even
# though everything below succeeded. Reset before any control flow
# touches `$LASTEXITCODE` again.
$global:LASTEXITCODE = 0
if ($legacyServicePresent) {
    Write-Warning @"
A legacy 'inferd-daemon' Windows service is registered. The new install
runs the daemon as a per-user Startup process and does NOT use SCM.
The legacy service does not own the named-pipe paths so the install
will succeed, but you will see a stale 'STOPPED' entry in 'sc.exe query'
until you remove it.

The v0.2.1 service SDDL strips DELETE/WRITE_DAC from Administrators,
so 'sc.exe delete' is blocked even when elevated. Use the included
helper instead — it self-elevates and rewrites the registry-key DACL:
    powershell -ExecutionPolicy Bypass -File cleanup-legacy-service.ps1
Then reboot to flush the SCM cache.
"@
}

$startupDir = [Environment]::GetFolderPath("Startup")
if (-not (Test-Path $startupDir)) {
    Write-Error "Startup folder not found at $startupDir"
}
$shortcutPath = Join-Path $startupDir $ShortcutName

# Shortcut arguments. Mirrors the launchd plist and systemd unit:
# explicit lock, infer pipe, embed enabled, explicit embed pipe. The
# admin pipe falls back to the daemon's platform default. Backend
# selection comes from %USERPROFILE%\.inferd\config.json (auto-written
# on first boot).
$shortcutArgs = @(
    "--lock",       "`"$LockPath`"",
    "--pipe",       "`"$PipePath`"",
    "--embed",
    "--embed-addr", "`"$EmbedPipePath`""
) -join " "

Write-Host "Creating Startup shortcut: $shortcutPath"
$wsh = New-Object -ComObject WScript.Shell
$lnk = $wsh.CreateShortcut($shortcutPath)
$lnk.TargetPath       = $BinaryPath
$lnk.Arguments        = $shortcutArgs
$lnk.WorkingDirectory = $installDir
# 7 = minimized. Daemon writes to log files; we don't need the console.
$lnk.WindowStyle      = 7
$lnk.Description      = "inferd local inference daemon. https://github.com/3rg0n/inferd"
$lnk.Save()

if (-not $NoStart) {
    # Stop any prior instance launched from a previous install before we
    # spawn a fresh one — best-effort. The single-instance lock will
    # reject a second daemon anyway, but stopping cleanly avoids a
    # confusing boot-time error in the activity log.
    Get-Process -Name "inferd-daemon" -ErrorAction SilentlyContinue |
        ForEach-Object {
            Write-Host "Stopping running inferd-daemon (PID $($_.Id))"
            Stop-Process -Id $_.Id -Force
        }

    Write-Host "Launching daemon: $BinaryPath $shortcutArgs"
    Start-Process -FilePath $BinaryPath `
                  -ArgumentList $shortcutArgs `
                  -WorkingDirectory $installDir `
                  -WindowStyle Hidden
}

Write-Host ""
Write-Host "Done."
Write-Host "  Binary:     $BinaryPath"
Write-Host "  Shortcut:   $shortcutPath"
Write-Host "  Logs:       $LogDir\inferd.ndjson"
Write-Host "  Infer pipe: $PipePath"
Write-Host "  Embed pipe: $EmbedPipePath"
Write-Host ""
Write-Host "On first boot the daemon writes %USERPROFILE%\.inferd\config.json"
Write-Host "(if absent) and pulls the configured generate + embed models into"
Write-Host "the CAS store. Watch progress with: inferdctl watch"
Write-Host ""
Write-Host "Verify status:    inferdctl status"
Write-Host "Uninstall:        .\uninstall.ps1"
