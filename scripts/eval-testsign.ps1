#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
Evaluation only: test-signs the RaplPkg driver and registers it as a demand-start kernel service.

.DESCRIPTION
For owner-approved evaluation on a single device when no Microsoft-signed build is available.
Never use this on production or shared machines.

The script:
1. creates (or reuses) a non-exportable self-signed code signing certificate in
   LocalMachine\My and trusts it in LocalMachine\Root and LocalMachine\TrustedPublisher;
2. copies the driver, rapl-bridge.exe and win-mmio-msr-cli.exe to InstallRoot and signs the
   driver copy with SHA-256;
3. copies the signed driver to System32\drivers and creates the kernel service rapl-pkg
   (demand start, not started);
4. enables test signing (bcdedit /set testsigning on). A reboot is required.

The script never disables Secure Boot, memory integrity (HVCI) or Windows Defender Application
Control. If a policy on the device still blocks the driver, `sc.exe start rapl-pkg` fails and
the Microsoft-Windows-CodeIntegrity/Operational event log shows the reason.

The machine-trusted test certificate is a security risk. Remove the setup with
scripts\eval-rollback.ps1 when the evaluation ends.

.PARAMETER DriverDir
Directory with rapl-pkg.sys, for example the extracted CI artifact driver-package-x64-unsigned\rapl-pkg.

.PARAMETER UserModeDir
Directory with rapl-bridge.exe and win-mmio-msr-cli.exe (CI artifact user-mode-x64 or a release build).

.EXAMPLE
.\scripts\eval-testsign.ps1 -DriverDir .\eval\driver-package-x64-unsigned\rapl-pkg -UserModeDir .\eval\user-mode-x64
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$DriverDir,
    [Parameter(Mandatory)][string]$UserModeDir,
    [string]$InstallRoot = (Join-Path $env:ProgramFiles 'WinMmioMsrEval'),
    [string]$Subject = 'CN=win-mmio-msr-cli eval test signing'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$service = 'rapl-pkg'
$systemDriver = Join-Path $env:SystemRoot 'System32\drivers\rapl-pkg.sys'

$existing = Get-Service -Name $service -ErrorAction SilentlyContinue
if ($existing -and $existing.Status -ne 'Stopped') {
    throw "Service $service is $($existing.Status); stop it before redeploying."
}

$cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object Subject -EQ $Subject | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject `
        -CertStoreLocation Cert:\LocalMachine\My -KeyExportPolicy NonExportable `
        -HashAlgorithm SHA256 -NotAfter (Get-Date).AddDays(90)
}
foreach ($storeName in 'Root', 'TrustedPublisher') {
    $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($storeName, 'LocalMachine')
    $store.Open('ReadWrite')
    try {
        $store.Add([System.Security.Cryptography.X509Certificates.X509Certificate2]::new($cert.RawData))
    } finally {
        $store.Close()
    }
}
Write-Output "Test certificate: $($cert.Thumbprint) (expires $($cert.NotAfter.ToString('yyyy-MM-dd')))"

New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $DriverDir 'rapl-pkg.sys') -Destination $InstallRoot -Force
foreach ($exe in 'rapl-bridge.exe', 'win-mmio-msr-cli.exe') {
    Copy-Item -LiteralPath (Join-Path $UserModeDir $exe) -Destination $InstallRoot -Force
}

$signed = Join-Path $InstallRoot 'rapl-pkg.sys'
$signature = Set-AuthenticodeSignature -FilePath $signed -Certificate $cert -HashAlgorithm SHA256
if ($signature.Status -ne 'Valid') {
    throw "Signing failed: $($signature.StatusMessage)"
}
Copy-Item -LiteralPath $signed -Destination $systemDriver -Force
Write-Output "Driver SHA-256: $((Get-FileHash -LiteralPath $systemDriver -Algorithm SHA256).Hash)"

if (-not $existing) {
    & sc.exe create $service type= kernel start= demand error= normal binPath= 'System32\drivers\rapl-pkg.sys' DisplayName= 'RaplPkg RAPL access driver (evaluation)' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe create failed with exit code $LASTEXITCODE." }
}

& bcdedit.exe /set testsigning on | Out-Null
if ($LASTEXITCODE -ne 0) { throw "bcdedit failed with exit code $LASTEXITCODE." }

Write-Output 'Done. Reboot, then run: sc.exe start rapl-pkg'
Write-Output "Read-only check: & '$InstallRoot\win-mmio-msr-cli.exe' status --bridge '$InstallRoot\rapl-bridge.exe'"
