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

Get-Process -Name "inferd-daemon" -ErrorAction SilentlyContinue |
    ForEach-Object {
        Write-Host "Stopping inferd-daemon (PID $($_.Id))"
        Stop-Process -Id $_.Id -Force
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
    Write-Host "Purge complete. Models in %LOCALAPPDATA%\models\ left intact."
} else {
    Write-Host ""
    Write-Host "Daemon stopped and Startup entry removed."
    Write-Host "Binary, logs, and config preserved. Re-run with -Purge to delete them."
}
