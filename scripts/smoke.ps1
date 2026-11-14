$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot '..\target\release\win-mmio-msr-cli.exe'
if (-not (Test-Path $exe)) { throw "Bitte zuerst scripts\build.ps1 ausfuehren." }
& $exe status --mock
if ($LASTEXITCODE -ne 0) { throw "status --mock fehlgeschlagen" }
& $exe plan --mock --pl1 55 --pl2 157
if ($LASTEXITCODE -ne 0) { throw "plan --mock fehlgeschlagen" }
Write-Host 'Simulation erfolgreich. Keine echten Hardware-Schreibzugriffe.'
