#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
Removes the evaluation setup created by eval-testsign.ps1.

.DESCRIPTION
Stops and deletes the rapl-pkg service, deletes System32\drivers\rapl-pkg.sys and InstallRoot,
removes the test certificate (with its private key) from LocalMachine\My, Root and
TrustedPublisher and disables test signing. A reboot is required. Safe to run more than once.
#>
[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:ProgramFiles 'WinMmioMsrEval'),
    [string]$Subject = 'CN=win-mmio-msr-cli eval test signing'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$service = 'rapl-pkg'
if (Get-Service -Name $service -ErrorAction SilentlyContinue) {
    & sc.exe stop $service | Out-Null
    & sc.exe delete $service | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe delete failed with exit code $LASTEXITCODE." }
}
Remove-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\drivers\rapl-pkg.sys') -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $InstallRoot -Recurse -Force -ErrorAction SilentlyContinue

foreach ($storeName in 'My', 'Root', 'TrustedPublisher') {
    Get-ChildItem "Cert:\LocalMachine\$storeName" | Where-Object Subject -EQ $Subject |
        Remove-Item -DeleteKey:($storeName -eq 'My')
}

& bcdedit.exe /set testsigning off | Out-Null
if ($LASTEXITCODE -ne 0) { throw "bcdedit failed with exit code $LASTEXITCODE." }

Write-Output 'Evaluation setup removed. Reboot to deactivate test signing.'
