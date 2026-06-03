# cleanup-legacy-service.ps1 — remove the legacy 'inferd-daemon' SCM
# service registered by pre-v0.2.2 install.ps1.
#
# Background: the v0.2.1 installer registered an SCM service with a
# hardened SDDL that stripped DELETE / WRITE_DAC / WRITE_OWNER from
# Administrators. Result: `sc.exe delete inferd-daemon` returns
# 'Access is denied' (5) even when run elevated. The new
# install.ps1 (Startup-folder shortcut, per-user) does not use SCM
# at all, but the zombie service registration sticks around until
# explicitly cleaned up.
#
# Strategy:
#   1. Self-elevate via UAC prompt if not already elevated.
#   2. Stop the service (best effort — sc.exe stop usually succeeds
#      even when delete is blocked, because STOP rights survived).
#   3. Take ownership of the registry key
#      HKLM:\SYSTEM\CurrentControlSet\Services\inferd-daemon and
#      grant Administrators FullControl on it.
#   4. Delete the registry key. SCM will not see the service after
#      the next reboot.
#   5. Reboot is required to flush the SCM cache. The script prints
#      a clear instruction; it does NOT reboot automatically.
#
# Run from an unelevated shell:
#   powershell -ExecutionPolicy Bypass -File cleanup-legacy-service.ps1
# UAC will prompt for elevation; click Yes.

[CmdletBinding()]
param(
    [string]$ServiceName = "inferd-daemon"
)

$ErrorActionPreference = "Stop"

function Test-Elevation {
    $id  = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $wp  = New-Object System.Security.Principal.WindowsPrincipal($id)
    return $wp.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Elevation)) {
    Write-Host "Re-launching elevated (UAC prompt incoming)..."
    $argList = @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", "`"$PSCommandPath`"",
        "-ServiceName", $ServiceName
    )
    Start-Process -FilePath "powershell.exe" `
                  -ArgumentList $argList `
                  -Verb RunAs `
                  -Wait
    exit $LASTEXITCODE
}

Write-Host "Elevated. Cleaning up legacy SCM service '$ServiceName'."

# Step 1: best-effort stop. The bad SDDL granted SERVICE_STOP to
# Authenticated Users so this will succeed even if delete cannot.
& sc.exe stop $ServiceName 2>&1 | Out-Null

# Step 2: locate the registry key. It may already be gone (a previous
# run may have completed step 4 but the SCM cache still holds the
# entry pending reboot).
$regPath = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
$keyExists = Test-Path $regPath

if (-not $keyExists) {
    Write-Host "Registry key already absent at $regPath."
    Write-Host "If 'sc.exe query $ServiceName' still lists the service, it is"
    Write-Host "an SCM cache entry that will clear on next reboot."
    return
}

# Step 3: take ownership and grant Administrators FullControl on the
# registry key. The current Administrators ACE may have been stripped
# of the rights needed to delete the key, so we rewrite it.
Write-Host "Taking ownership of $regPath ..."

$adminGroup = New-Object System.Security.Principal.SecurityIdentifier(
    [System.Security.Principal.WellKnownSidType]::BuiltinAdministratorsSid,
    $null
)

# Open the key with TakeOwnership permission to rewrite Owner.
$reg = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
    "SYSTEM\CurrentControlSet\Services\$ServiceName",
    [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
    [System.Security.AccessControl.RegistryRights]::TakeOwnership
)
$acl = $reg.GetAccessControl([System.Security.AccessControl.AccessControlSections]::Owner)
$acl.SetOwner($adminGroup)
$reg.SetAccessControl($acl)
$reg.Close()

# Re-open with ChangePermissions to rewrite DACL.
$reg = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
    "SYSTEM\CurrentControlSet\Services\$ServiceName",
    [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
    [System.Security.AccessControl.RegistryRights]::ChangePermissions
)
$acl  = $reg.GetAccessControl([System.Security.AccessControl.AccessControlSections]::Access)
$rule = New-Object System.Security.AccessControl.RegistryAccessRule(
    $adminGroup,
    [System.Security.AccessControl.RegistryRights]::FullControl,
    "ContainerInherit,ObjectInherit",
    "None",
    "Allow"
)
$acl.AddAccessRule($rule)
$reg.SetAccessControl($acl)
$reg.Close()

# Step 4: delete the key.
Write-Host "Deleting $regPath ..."
Remove-Item -Path $regPath -Recurse -Force

# Step 5: report. SCM caches the service entry until reboot.
Write-Host ""
Write-Host "Done. Registry key removed."
Write-Host ""
Write-Host "NOTE: SCM caches the service entry in memory. 'sc.exe query"
Write-Host "      $ServiceName' may still list it as STOPPED until a full"
Write-Host "      reboot. The cached entry has no ImagePath and cannot"
Write-Host "      auto-start, so it does NOT interfere with the new"
Write-Host "      Startup-folder install path. A reboot will fully clear it."
Write-Host ""
Write-Host "Next:"
Write-Host "  - Run the new installer (no elevation needed):"
Write-Host "      powershell -ExecutionPolicy Bypass -File install.ps1 -SourceBinary <path>"
Write-Host "  - Optional: Restart-Computer to flush the SCM cache."
