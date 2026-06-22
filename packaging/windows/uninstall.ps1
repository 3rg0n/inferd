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
    Write-Host "Purge complete. Models in %LOCALAPPDATA%\models\ left intact."
} else {
    Write-Host ""
    Write-Host "Daemon stopped and Startup entry removed."
    Write-Host "Binary, logs, and config preserved. Re-run with -Purge to delete them."
}
