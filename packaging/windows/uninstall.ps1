# inferd-daemon Windows per-user uninstaller.
#
# Run as the current user — no elevation required:
#   powershell -ExecutionPolicy Bypass -File uninstall.ps1
#
# Removes the Startup shortcut, stops the running daemon, and (with
# -Purge) deletes the staged binary, lock, logs, and config. Models
# in the CAS store are NEVER removed by this script — re-pulling the
# multi-GB blobs is a slow operation and the operator can delete
# %LOCALAPPDATA%\models\ themselves if they really want.

[CmdletBinding()]
param(
    [string]$BinaryPath   = "$env:LOCALAPPDATA\inferd\inferd-daemon.exe",
    [string]$LockPath     = "$env:LOCALAPPDATA\inferd\inferd.lock",
    [string]$LogDir       = "$env:LOCALAPPDATA\inferd\logs",
    [string]$ShortcutName = "inferd-daemon.lnk",
    [switch]$Purge
)

$ErrorActionPreference = "Stop"

$startupDir   = [Environment]::GetFolderPath("Startup")
$shortcutPath = Join-Path $startupDir $ShortcutName

if (Test-Path $shortcutPath) {
    Write-Host "Removing Startup shortcut: $shortcutPath"
    Remove-Item -Path $shortcutPath -Force
} else {
    Write-Host "No Startup shortcut at $shortcutPath (already removed?)"
}

$stopped = Get-Process -Name "inferd-daemon" -ErrorAction SilentlyContinue
$stopped | ForEach-Object {
    Write-Host "Stopping inferd-daemon (PID $($_.Id))"
    Stop-Process -Id $_.Id -Force
}
# Wait for the OS to release the process's file handles before the
# -Purge below tries to delete the install dir. A killed process holds
# exclusive locks on its loaded DLLs (cublas/cudart/ggml/llama) for a
# short window after exit; deleting too soon fails with "Access to the
# path '...dll' is denied". Bounded wait until no daemon remains.
if ($stopped) {
    for ($i = 0; $i -lt 50; $i++) {
        Start-Sleep -Milliseconds 100
        if (-not (Get-Process -Name "inferd-daemon" -ErrorAction SilentlyContinue)) { break }
    }
    Start-Sleep -Milliseconds 300  # extra grace for handle release after exit
}

# Lock file may linger if the daemon was killed hard. Clean it so the
# next install boots without a stale-lock complaint.
if (Test-Path $LockPath) {
    try { Remove-Item -Path $LockPath -Force } catch { }
}

if ($Purge) {
    $installDir = Split-Path -Parent $BinaryPath
    if (Test-Path $installDir) {
        Write-Host "Purging $installDir"
        Remove-Item -Path $installDir -Recurse -Force
    }
    $configDir = Join-Path $env:USERPROFILE ".inferd"
    if (Test-Path $configDir) {
        Write-Host "Purging $configDir"
        Remove-Item -Path $configDir -Recurse -Force
    }
    [System.Environment]::SetEnvironmentVariable("INFERD_LOG",     $null, "User")
    [System.Environment]::SetEnvironmentVariable("INFERD_LOG_DIR", $null, "User")

    # install.ps1 appends $installDir to the user PATH so the `inferdctl`
    # commands it prints resolve (issue #58). Purge removes the directory, so
    # leaving the entry behind would strand a dead path in HKCU\Environment.
    # Read the User scope specifically — $env:PATH is machine + user
    # concatenated, and writing that back to User scope would copy every
    # system entry into the user's PATH.
    # install.ps1 declines to append when PATH is REG_EXPAND_SZ, because the
    # .NET accessors can't round-trip it (getter expands, setter writes REG_SZ).
    # The same hazard applies to removal, so decline symmetrically: if the kind
    # is ExpandString then this installer never added the entry in the first
    # place, and rewriting it here would flatten the user's %VAR% references.
    $pathKindOk = $true
    try {
        $envKey = Get-Item "HKCU:\Environment" -ErrorAction Stop
        if (($envKey.GetValue("Path", $null) -ne $null) -and
            ($envKey.GetValueKind("Path") -eq "ExpandString")) {
            $pathKindOk = $false
        }
    } catch { $pathKindOk = $false }

    $userPath = [System.Environment]::GetEnvironmentVariable("PATH", "User")
    if ($userPath -and $pathKindOk) {
        # Remove ONLY our own entry. Empty segments are left alone even though
        # they are junk: they belong to whoever put them there, and rewriting
        # parts of PATH this installer did not add is not this script's job.
        $target = $installDir.TrimEnd('\')
        $kept = @($userPath.Split(';') | Where-Object { $_.TrimEnd('\') -ne $target })
        $rebuilt = $kept -join ';'
        if ($rebuilt -ne $userPath) {
            Write-Host "Removing $installDir from your user PATH"
            [System.Environment]::SetEnvironmentVariable("PATH", $rebuilt, "User")
        }
    } elseif (-not $pathKindOk) {
        Write-Warning "not touching your user PATH: it is stored as REG_EXPAND_SZ, which these scripts cannot rewrite without flattening its %VAR% references. If $installDir is on it, remove the entry yourself."
    }
    Write-Host "Purge complete. Models in %LOCALAPPDATA%\models\ left intact."
} else {
    Write-Host ""
    Write-Host "Daemon stopped and Startup entry removed."
    Write-Host "Binary, logs, and config preserved. Re-run with -Purge to delete them."
}
