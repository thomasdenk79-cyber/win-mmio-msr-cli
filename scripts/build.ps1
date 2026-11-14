#Requires -Version 5.1
<#
.SYNOPSIS
Checks, tests and builds the workspace for x64 Windows; optionally builds the driver package.

.DESCRIPTION
Runs the CI checks locally: rustfmt, Clippy with warnings as errors, all tests
and a locked release build for x86_64-pc-windows-msvc. The binaries are written
to target\x86_64-pc-windows-msvc\release. The script stops at the first failing
step.

With -Driver it also builds the unsigned driver package with the default
(read-only) gates into target\driver\x64\Release\rapl-pkg. This needs Visual
Studio 2022 with the C++ workload, the Spectre-mitigated MSVC libraries and the
WDK 10.0.26100. The package cannot be loaded without a Microsoft signature; see
driver\README.md.

.PARAMETER Driver
Also build the driver package with the 64-bit MSBuild.

.EXAMPLE
.\scripts\build.ps1

.EXAMPLE
.\scripts\build.ps1 -Driver
#>
[CmdletBinding()]
param([switch]$Driver)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Target = 'x86_64-pc-windows-msvc'
$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path

function Invoke-Checked([string]$Name, [scriptblock]$Command) {
    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed with exit code $LASTEXITCODE." }
}

function Find-MsBuild {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw 'vswhere.exe not found. Install Visual Studio 2022 with the C++ workload and the WDK.'
    }
    # The WDK 26100 build tasks load only the x64 InfVerif and ApiValidator.
    $found = @(& $vswhere -latest -products * -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\amd64\MSBuild.exe')
    if ($LASTEXITCODE -ne 0 -or $found.Count -eq 0) { throw 'The 64-bit MSBuild of Visual Studio was not found.' }
    $found[0]
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo not found. Install Rust from https://rustup.rs/ first.'
}

Push-Location -LiteralPath $Root
try {
    Invoke-Checked 'Format check' { cargo fmt --all -- --check }
    Invoke-Checked 'Clippy' { cargo clippy --workspace --all-targets --locked --target $Target -- -D warnings }
    Invoke-Checked 'Tests' { cargo test --workspace --all-targets --locked --target $Target }
    Invoke-Checked 'Release build' { cargo build --workspace --release --locked --target $Target }
    if ($Driver) {
        $msbuild = Find-MsBuild
        Invoke-Checked 'Driver package' {
            & $msbuild driver\rapl-pkg.vcxproj -m -nologo -warnaserror -p:Configuration=Release -p:Platform=x64
        }
    }
}
finally {
    Pop-Location
}

Write-Host "Binaries: $(Join-Path $Root "target\$Target\release")"
if ($Driver) {
    Write-Host "Unsigned driver package: $(Join-Path $Root 'target\driver\x64\Release\rapl-pkg')"
}
