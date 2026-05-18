# inferd-daemon Windows service installer.
#
# Run elevated:
#   powershell -ExecutionPolicy Bypass -File install.ps1
#
# Installs the daemon as a Windows service running as the current user.
# Uses sc.exe rather than New-Service to keep the script Windows
# PowerShell + PowerShell 7 compatible.
#
# THREAT_MODEL F-16: Windows service hardening is constrained relative
# to systemd / launchd. The controls applied here:
#   - Service runs as NT AUTHORITY\NetworkService (not LocalSystem).
#   - Recovery actions reset the failure counter on success and
#     restart on failure with a 2s delay.
#   - Custom service DACL via sc.exe sdset: SYSTEM + Administrators get
#     full control; Authenticated Users get query/interrogate only,
#     no stop / start / pause / config-change. This stops a non-admin
#     local user from killing or replacing the daemon out from under
#     other middlewares on the same machine.
#   - Service description points at the upstream documentation so
#     ops staff can find the contract.

[CmdletBinding()]
param(
    [string]$ServiceName = "inferd-daemon",
    [string]$DisplayName = "inferd local inference daemon",
    [string]$BinaryPath = "$env:LOCALAPPDATA\inferd\inferd-daemon.exe",
    [string]$LockPath = "$env:LOCALAPPDATA\inferd\inferd.lock",
    [string]$PipePath = "\\.\pipe\inferd-infer",
    [string]$LogDir = "$env:LOCALAPPDATA\inferd\logs",
    [string]$Backend = "mock"
)

$ErrorActionPreference = "Stop"

# Pre-flight.
if (-not (Test-Path $BinaryPath)) {
    Write-Error "inferd-daemon binary not found at $BinaryPath. Copy it there first or pass -BinaryPath."
}
$logDirParent = Split-Path -Parent $LogDir
if (-not (Test-Path $logDirParent)) {
    New-Item -ItemType Directory -Path $logDirParent -Force | Out-Null
}
if (-not (Test-Path $LogDir)) {
    New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
}

# Build the BinPath argument. sc.exe requires the entire command in one
# quoted string with embedded quotes around paths that contain spaces.
$bin = '"' + $BinaryPath + '"' + " --backend $Backend " +
       "--lock `"$LockPath`" " +
       "--pipe `"$PipePath`""

Write-Host "Installing service '$ServiceName' from $BinaryPath..."

# If service exists, remove first.
& sc.exe query $ServiceName 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0) {
    Write-Host "Service exists; stopping + deleting."
    & sc.exe stop $ServiceName 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    & sc.exe delete $ServiceName 2>&1 | Out-Null
}

& sc.exe create $ServiceName `
    binPath= $bin `
    DisplayName= $DisplayName `
    start= auto `
    obj= "NT AUTHORITY\NetworkService" `
    | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "sc.exe create failed (exit $LASTEXITCODE)"
}

# Description and recovery.
& sc.exe description $ServiceName "Local inference daemon. See https://github.com/3rg0n/inferd."
& sc.exe failure $ServiceName reset= 60 actions= restart/2000/restart/2000/restart/2000

# Custom service DACL (THREAT_MODEL F-16, Windows half).
#
# SDDL service-rights breakdown:
#   CC SERVICE_QUERY_CONFIG       LC SERVICE_QUERY_STATUS
#   SW SERVICE_ENUMERATE_DEPENDENTS  LO SERVICE_INTERROGATE
#   RP SERVICE_START              WP SERVICE_STOP
#   DT SERVICE_PAUSE_CONTINUE     CR SERVICE_USER_DEFINED_CONTROL
#   RC READ_CONTROL
#
# Layout:
#   D:                                            -- DACL
#     (A;;CCLCSWRPWPDTLOCRRC;;;SY)                -- LOCAL_SYSTEM: full
#     (A;;CCLCSWRPWPDTLOCRRC;;;BA)                -- Administrators: full
#     (A;;CCLCLORC;;;AU)                          -- Auth Users: query/interrogate only
#   S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)     -- SACL: audit failures from anyone
#
# This denies SERVICE_STOP / SERVICE_START / SERVICE_PAUSE_CONTINUE /
# SERVICE_CHANGE_CONFIG to Authenticated Users. They retain
# SERVICE_QUERY_STATUS / SERVICE_QUERY_CONFIG / SERVICE_INTERROGATE
# / READ_CONTROL — enough to know it's running, not enough to mess
# with it.
$sddl = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCLCSWRPWPDTLOCRRC;;;BA)(A;;CCLCLORC;;;AU)" +
        "S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)"
& sc.exe sdset $ServiceName $sddl | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Warning "sc.exe sdset exit=$LASTEXITCODE; service installed but DACL not hardened. Verify with sc.exe sdshow $ServiceName"
}

# Environment variables for the service. INFERD_LOG_DIR controls the
# activity log location.
$envBlock = "INFERD_LOG=info`0INFERD_LOG_DIR=$LogDir`0`0"
$envBytes = [System.Text.Encoding]::Unicode.GetBytes($envBlock)
$regPath  = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
Set-ItemProperty -Path $regPath -Name Environment -Value $envBytes -Type MultiString -Force

Write-Host "Starting service..."
& sc.exe start $ServiceName | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Warning "sc.exe start exit=$LASTEXITCODE; check Event Viewer + ${LogDir}\inferd.ndjson"
}

Write-Host "Done. Verify with: sc.exe query $ServiceName"
Write-Host "Activity log: $LogDir\inferd.ndjson"
