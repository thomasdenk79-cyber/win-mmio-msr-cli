#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
End-to-end test of install-service.ps1 and rapl-watchdog against the real Service Control Manager.

.DESCRIPTION
Runs only on disposable GitHub Actions machines without the RaplPkg driver. It
checks that the installer refuses a user-writable install directory, deploys the
binaries, verifies that service start parameters are refused (service exit code
1), lets the service halt because the driver is missing (exit code 2), checks
the audit trail, reinstalls with PowerShell 7 and removes everything again.
No register is read or written: without the driver every bridge call fails.
#>
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$BinaryDir)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ServiceName = 'RaplWatchdog'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This test installs and removes a service; it runs only in GitHub Actions.' }
if (Get-Service -Name 'rapl-pkg' -ErrorAction SilentlyContinue) { throw 'The rapl-pkg driver is installed; refusing to run.' }
if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) { throw "$ServiceName already exists; refusing to replace it." }

$BinaryDir = (Resolve-Path -LiteralPath $BinaryDir).Path
$installer = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..\scripts\install-service.ps1')).Path
$root = Join-Path $env:ProgramFiles 'WinMmioMsr-smoke'
$data = Join-Path $env:ProgramData 'WinMmioMsr-smoke'
$unsafe = Join-Path $env:ProgramData 'WinMmioMsr-smoke-unsafe'
$log = Join-Path $data 'rapl-audit.jsonl'
$settings = @{ AuditLog = $log; BackupDir = (Join-Path $data 'backups'); IntervalMs = 1000; MaxConsecutiveErrors = 2 }

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "Assertion failed: $Message" }
}

function Start-AndWaitForExit([string[]]$Arguments) {
    & sc.exe start $ServiceName @Arguments | Out-Host
    Write-Host "sc.exe start exit code: $LASTEXITCODE"
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 500
        $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
        if ((Get-Date) -gt $deadline) { throw "$ServiceName did not stop within 60 s (state $($service.State))." }
    } while ($service.State -ne 'Stopped')
    Write-Host "Stopped: ExitCode=$($service.ExitCode) ServiceSpecificExitCode=$($service.ServiceSpecificExitCode)"
    $service
}

function Get-WatchdogRecord {
    @(Get-Content -LiteralPath $log | ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.source -eq 'rapl-watchdog' })
}

try {
    Write-Host '--- 1. A user-writable install directory is refused'
    New-Item -ItemType Directory -Path $unsafe -Force | Out-Null
    & icacls.exe $unsafe /grant '*S-1-5-32-545:(OI)(CI)M' | Out-Null
    Assert-That ($LASTEXITCODE -eq 0) 'icacls could not grant Users modify access'
    Copy-Item -Path (Join-Path $BinaryDir 'rapl-*.exe') -Destination $unsafe
    $refusal = ''
    try { & $installer -InstallRoot $unsafe @settings } catch { $refusal = $_.Exception.Message }
    Write-Host "Installer: $refusal"
    Assert-That ($refusal -match '^Refusing') 'install-service.ps1 accepted a user-writable install directory'
    Assert-That (-not (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)) 'service registered despite the refusal'

    Write-Host '--- 2. Deploy into a new protected directory'
    & $installer -InstallRoot $root -SourceDir $BinaryDir @settings
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
    Assert-That ($service.StartMode -eq 'Manual' -and $service.StartName -eq 'LocalSystem') 'unexpected service settings'
    foreach ($dir in $root, $data, $settings.BackupDir) {
        Assert-That (Get-Acl -LiteralPath $dir).AreAccessRulesProtected "$dir inherits permissions"
    }

    Write-Host '--- 3. Start parameters are refused with exit code 1'
    $service = Start-AndWaitForExit @('unexpected')
    Assert-That ($service.ExitCode -eq 1066 -and $service.ServiceSpecificExitCode -eq 1) 'start parameters were not refused'
    Assert-That (@(Get-WatchdogRecord | Where-Object { $_.event -eq 'service_start_refused' }).Count -eq 1) 'refusal not audited'

    Write-Host '--- 4. Without the driver the watchdog halts with exit code 2'
    $service = Start-AndWaitForExit @()
    Assert-That ($service.ExitCode -eq 1066 -and $service.ServiceSpecificExitCode -eq 2) 'service did not halt with exit code 2'
    $events = Get-WatchdogRecord
    $events | Format-Table event, consecutive_errors, error, halt_reason -AutoSize -Wrap | Out-Host
    $names = @($events | ForEach-Object { $_.event })
    Assert-That ($names -contains 'service_started') 'service_started missing'
    Assert-That (@($names | Where-Object { $_ -eq 'service_error' }).Count -eq 2) 'expected two service_error records'
    $halt = @($events | Where-Object { $_.event -eq 'service_halted' })
    Assert-That ($halt.Count -eq 1 -and $halt[0].halt_reason -match '^2 consecutive errors') 'service_halted missing'
    Assert-That (@($names | Where-Object { $_ -match 'correction' }).Count -eq 0) 'unexpected correction attempt'

    Write-Host '--- 5. The CLI reads the service audit records'
    & (Join-Path $BinaryDir 'win-mmio-msr-cli.exe') audit $log | Out-Host
    Assert-That ($LASTEXITCODE -eq 0) 'win-mmio-msr-cli audit failed'

    Write-Host '--- 6. Reinstall over the stopped service with PowerShell 7'
    $backups = Join-Path $data 'backups-pwsh'
    & pwsh -NoProfile -NonInteractive -File $installer -InstallRoot $root -SourceDir $BinaryDir `
        -AuditLog $log -BackupDir $backups -IntervalMs 1000 -MaxConsecutiveErrors 2
    Assert-That ($LASTEXITCODE -eq 0) 'reinstall with PowerShell 7 failed'
    Assert-That (Get-Acl -LiteralPath $backups).AreAccessRulesProtected 'PowerShell 7 created an unprotected directory'
    $service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
    Assert-That ($service.PathName -match 'backups-pwsh') 'reinstall did not update the service command line'
}
finally {
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue
        & sc.exe delete $ServiceName | Out-Null
    }
    foreach ($dir in $root, $data, $unsafe) {
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
    }
}
Write-Host 'Service smoke test passed.'
exit 0
