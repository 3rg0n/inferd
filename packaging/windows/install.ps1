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
#   - Pipes at                     \\.\pipe\inferd            (generation)
#                                  \\.\pipe\inferd-infer-embed (embeddings)
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
    [string]$PipePath      = "\\.\pipe\inferd",
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

# Stop any running daemon BEFORE staging the binary. On Windows a
# running process holds an exclusive lock on its own .exe, so an
# upgrade-over-running-install would fail at the Copy-Item below with
# "process cannot access the file ... being used by another process".
# (The single-instance lock would also reject a second daemon at
# launch, but that's after staging — too late.) Best-effort: give the
# process a moment to release the file handle after exit.
$running = Get-Process -Name "inferd-daemon" -ErrorAction SilentlyContinue
if ($running) {
    foreach ($proc in $running) {
        Write-Host "Stopping running inferd-daemon (PID $($proc.Id)) before staging"
        Stop-Process -Id $proc.Id -Force
    }
    # Wait for the OS to release the exe lock (handle close is async).
    for ($i = 0; $i -lt 50; $i++) {
        Start-Sleep -Milliseconds 100
        if (-not (Get-Process -Name "inferd-daemon" -ErrorAction SilentlyContinue)) { break }
    }
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

    # Stage the CLI too (issue #58). This script's closing message tells the
    # operator to run `inferdctl status`, and on an airgapped build
    # `inferdctl import` is the ONLY way a model reaches the store — so the
    # CLI sits on the critical path of the runbook we print. Both release
    # archives ship it in the same directory as the daemon.
    #
    # inferd-http is deliberately NOT staged: it is a separate, user-launched
    # consumer (ADR 0020/0014), not part of the daemon install, and nothing
    # this installer prints asks the operator to run it.
    $sourceCli = Join-Path $sourceDir "inferdctl.exe"
    if (Test-Path $sourceCli) {
        $cliPath = Join-Path $installDir "inferdctl.exe"
        Write-Host "Staging $sourceCli -> $cliPath"
        Copy-Item -Path $sourceCli -Destination $cliPath -Force
    } else {
        Write-Warning "no inferdctl.exe next to $SourceBinary; the CLI commands printed below will not resolve until you copy it into $installDir"
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

# Put the install dir on the user PATH so the `inferdctl ...` lines this
# script prints actually resolve (issue #58). Staging the CLI without this
# would still leave the operator hunting for it.
#
# Read the User scope specifically, never $env:PATH — the process PATH is
# machine PATH + user PATH concatenated, so writing it back to User scope
# would copy every system entry into the user's own PATH and permanently
# double it. This is the trap that makes naive PATH appends destructive.
$pathAddedToUser = $false
$userPath = [System.Environment]::GetEnvironmentVariable("PATH", "User")
if ($null -eq $userPath) { $userPath = "" }
# Compare case-insensitively (-contains is case-insensitive) and ignore
# trailing slashes so re-running the installer doesn't append a second copy.
$normalized = $userPath.Split(';') | ForEach-Object { $_.TrimEnd('\') }
if ($normalized -notcontains $installDir.TrimEnd('\')) {
    $newUserPath = if ($userPath -eq "") { $installDir } else { "$($userPath.TrimEnd(';'));$installDir" }

    # Two conditions make an append unsafe, and both end the same way: refuse,
    # and let the closing message fall back to the fully-qualified path. The
    # install is still complete and usable either way — the only thing lost is
    # the convenience of a bare `inferdctl`. Damaging a user's PATH to buy that
    # convenience is not a trade this installer gets to make.
    $refusal = ""

    # (a) HKCU\Environment's PATH is practically capped around 2048 chars; past
    # that, writing it can truncate and cost the user unrelated entries.
    if ($newUserPath.Length -gt 2000) {
        $refusal = "user PATH is $($userPath.Length) chars; not appending $installDir (risk of truncating existing entries)."
    }

    # (b) PATH may be stored as REG_EXPAND_SZ, i.e. holding literal
    # `%USERPROFILE%\bin`-style tokens that Windows expands per process. The
    # .NET accessors cannot round-trip that: the getter returns the *expanded*
    # string and the setter writes REG_SZ, so an append would silently bake
    # today's expansion in and downgrade the value kind, permanently losing the
    # user's indirection. Rewriting it losslessly means writing the registry
    # directly, which then needs a WM_SETTINGCHANGE broadcast that
    # SetEnvironmentVariable does for free — more machinery, and more ways to
    # damage PATH, than the convenience is worth. Detect and decline instead.
    if ($refusal -eq "") {
        try {
            $envKey = Get-Item "HKCU:\Environment" -ErrorAction Stop
            # No User-scope PATH at all: nothing to preserve, a fresh REG_SZ is
            # correct. Only an existing ExpandString is a problem.
            if (($envKey.GetValue("Path", $null) -ne $null) -and
                ($envKey.GetValueKind("Path") -eq "ExpandString")) {
                $refusal = "your user PATH is stored as REG_EXPAND_SZ (it contains %VAR% references); appending would expand and flatten them."
            }
        } catch {
            $refusal = "could not read HKCU\Environment to check the PATH value kind: $($_.Exception.Message)"
        }
    }

    if ($refusal -ne "") {
        Write-Warning "$refusal Use the full path shown below, or add $installDir to PATH yourself."
    } else {
        Write-Host "Adding $installDir to your user PATH"
        [System.Environment]::SetEnvironmentVariable("PATH", $newUserPath, "User")
        $pathAddedToUser = $true
    }
}
# Also for THIS process, so anything further down (and an operator who keeps
# using this same shell) can resolve the CLI without reopening a terminal.
if ($env:PATH.Split(';') -notcontains $installDir) {
    $env:PATH = "$env:PATH;$installDir"
}

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
# explicit lock, the single generation pipe, embed enabled. The admin
# pipe falls back to the daemon's platform default. Backend selection
# comes from %USERPROFILE%\.inferd\config.json (auto-written on first
# boot).
#
# v0.4 (ADR 0021): one generation surface on the neutral pipe
# (\\.\pipe\inferd) carrying typed content blocks + attachments — the
# old --v2 / --v2-addr flags are gone (v1 was folded into v2). The
# default config ships a vision projector (issue #30); a
# generation-only backend simply advertises vision:false on the same
# socket. --embed binds the embeddings pipe when the backend supports it.
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
    # Any prior instance was already stopped before staging (above), so
    # the single-instance lock is free for the fresh launch.
    Write-Host "Launching daemon: $BinaryPath $shortcutArgs"
    Start-Process -FilePath $BinaryPath `
                  -ArgumentList $shortcutArgs `
                  -WorkingDirectory $installDir `
                  -WindowStyle Hidden
}

Write-Host ""
Write-Host "Done."
Write-Host "  Binary:     $BinaryPath"
# Report the CLI the same way as the daemon, and name it as missing when it
# is (issue #58) rather than printing commands that won't resolve.
$cliPath = Join-Path $installDir "inferdctl.exe"
$cliStaged = Test-Path $cliPath
if ($cliStaged) {
    Write-Host "  CLI:        $cliPath"
} else {
    Write-Host "  CLI:        NOT INSTALLED (inferdctl.exe was not next to -SourceBinary)"
}
Write-Host "  Shortcut:   $shortcutPath"
Write-Host "  Logs:       $LogDir\inferd.ndjson"
Write-Host "  Generation pipe: $PipePath"
Write-Host "  Embed pipe:      $EmbedPipePath"
Write-Host ""
Write-Host "On first boot the daemon writes %USERPROFILE%\.inferd\config.json"
Write-Host "(if absent)."

# One installer ships in both release archives (ADR 0028), and only one of
# them can fetch models. Ask the binary rather than guessing: it prints its
# own build profile, so this message cannot drift from what got installed.
# `--version` just prints and exits — it takes no single-instance lock.
#
# Three outcomes, not two: if `--version` can't be read, say so rather
# than printing the networked message on a guess. Guessing "networked"
# on an airgapped install tells the operator to wait for a pull that
# will never start, which is the worst of the three messages to get
# wrong.
$profileText = ""
try { $profileText = (& $BinaryPath --version 2>&1 | Out-String) } catch { }

# How to spell the CLI in the lines below. A bare `inferdctl` is only
# correct once the dir is on PATH; when it isn't (long-PATH refusal, or a
# pre-existing PATH this script didn't touch and can't verify), print the
# fully-qualified path so every command is copy-pasteable as printed.
if ($cliStaged) {
    $cli = if ($pathAddedToUser -or ($normalized -contains $installDir.TrimEnd('\'))) {
        "inferdctl"
    } else {
        "`"$cliPath`""
    }
} else {
    # Not staged: point at the archive copy rather than a name that resolves
    # to nothing.
    $cli = ".\inferdctl.exe"
}

if ($profileText -match 'build profile: airgapped') {
    Write-Host "This is an AIRGAPPED build: no HTTPS client is linked, so it will"
    Write-Host "not fetch models. Import them from local files, then clear each"
    Write-Host "source_url in config.json:"
    Write-Host "  $cli import --name gemma-4-e4b <path.gguf>"
    Write-Host "See airgapped.md in the archive root for the full runbook."
} elseif ($profileText -match 'build profile: networked') {
    Write-Host "It then pulls the configured generate + embed models into the CAS"
    Write-Host "store. Watch progress with: $cli watch"
} else {
    Write-Host "Could not read the build profile from '$BinaryPath --version', so"
    Write-Host "this script can't tell whether it fetches models. Run:"
    Write-Host "  `"$BinaryPath`" --version"
    Write-Host "A 'networked' build pulls models on first boot ($cli watch);"
    Write-Host "an 'airgapped' build needs '$cli import' (see airgapped.md in"
    Write-Host "the archive root)."
}
Write-Host ""
Write-Host "Verify status:    $cli status"
Write-Host "Uninstall:        .\uninstall.ps1"
if (-not $cliStaged) {
    Write-Host ""
    Write-Host "NOTE: inferdctl.exe was not staged, so the commands above are shown"
    Write-Host "relative to the unpacked archive. Re-run this installer with"
    Write-Host "-SourceBinary pointing into the archive to install the CLI too."
} elseif ($pathAddedToUser) {
    Write-Host ""
    Write-Host "NOTE: $installDir was added to your user PATH. Already-open terminals"
    # ASCII only inside printed strings: powershell.exe reads this file as ANSI,
    # so a UTF-8 em-dash arrives as a curly quote (0x94 -> U+201D) which
    # terminates the string and breaks parsing. Harmless in comments and
    # here-strings, fatal here.
    Write-Host "inherited the old PATH - open a new one, or use the full path:"
    Write-Host "  `"$cliPath`" status"
}
