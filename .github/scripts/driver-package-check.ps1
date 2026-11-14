#Requires -Version 5.1
<#
.SYNOPSIS
    Checks the driver package built by driver/rapl-pkg.vcxproj (CI job driver-windows).
.DESCRIPTION
    Runs InfVerif in Windows Driver mode on the package folder, checks the stamped
    DriverVer and the x64 image, and prints SHA-256 hashes as build evidence.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PackageDir,
    [string]$ExpectedVersion = '0.3.0.0'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $PackageDir -PathType Container)) {
    throw "Package folder not found: $PackageDir"
}
$package = (Resolve-Path -LiteralPath $PackageDir).ProviderPath
$inf = Get-Item -LiteralPath (Join-Path $package 'rapl-pkg.inf')
Write-Output "Package folder: $package"
foreach ($name in 'rapl-pkg.sys', 'rapl-pkg.inf', 'rapl-pkg.cat') {
    $file = Join-Path $package $name
    if (-not (Test-Path -LiteralPath $file)) { throw "Missing $name in $package" }
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
    Write-Output ("{0,-13} {1,8} bytes  SHA256 {2}" -f $name, (Get-Item -LiteralPath $file).Length, $hash)
}

$driverVer = Select-String -LiteralPath $inf.FullName -Pattern '^\s*DriverVer\s*=' | Select-Object -First 1
if (-not $driverVer) { throw 'The stamped INF has no DriverVer' }
Write-Output "Stamped $($driverVer.Line.Trim())"
if ($driverVer.Line -notmatch [regex]::Escape(",$ExpectedVersion")) {
    throw "DriverVer does not carry version $ExpectedVersion"
}

$kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Tools'
$infVerif = Get-ChildItem -LiteralPath $kits -Recurse -Filter 'infverif.exe' |
    Where-Object { $_.DirectoryName -match '\\x64$' } |
    Sort-Object FullName -Descending |
    Select-Object -First 1
if (-not $infVerif) { throw "infverif.exe not found below $kits" }
& $infVerif.FullName /w /v $inf.FullName
if ($LASTEXITCODE -ne 0) { throw "InfVerif /w failed with exit code $LASTEXITCODE" }

$bytes = [System.IO.File]::ReadAllBytes((Join-Path $package 'rapl-pkg.sys'))
$peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
$machine = [BitConverter]::ToUInt16($bytes, $peOffset + 4)
if ($machine -ne 0x8664) { throw ('rapl-pkg.sys is not an x64 image (machine 0x{0:X4})' -f $machine) }
$signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $package 'rapl-pkg.sys')
Write-Output "Authenticode status of rapl-pkg.sys: $($signature.Status) (the CI build is unsigned)"
Write-Output 'Driver package check passed'
