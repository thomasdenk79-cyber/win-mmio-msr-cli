param(
  [Parameter(Mandatory=$true)][string]$InstallRoot,
  [string]$AuditLog = "$env:ProgramData\WinMmioMsr\rapl-audit.jsonl",
  [string]$BackupDir = "$env:ProgramData\WinMmioMsr\backups",
  [int]$Pl1 = 55,
  [int]$Pl2 = 157,
  [int]$IntervalMs = 5000
)
$ErrorActionPreference = 'Stop'
$exe = Join-Path $InstallRoot 'rapl-watchdog.exe'
if (-not (Test-Path $exe)) { throw "Missing $exe" }
New-Item -ItemType Directory -Force -Path (Split-Path $AuditLog), $BackupDir | Out-Null
$bin = '"{0}" run --bridge "{1}" --audit-log "{2}" --backup-dir "{3}" --pl1 {4} --pl2 {5} --interval-ms {6}' -f $exe,(Join-Path $InstallRoot 'rapl-bridge.exe'),$AuditLog,$BackupDir,$Pl1,$Pl2,$IntervalMs
if (Get-Service RaplWatchdog -ErrorAction SilentlyContinue) { Stop-Service RaplWatchdog -ErrorAction SilentlyContinue; sc.exe delete RaplWatchdog | Out-Null; Start-Sleep -Seconds 1 }
New-Service -Name RaplWatchdog -BinaryPathName $bin -DisplayName 'RAPL Limit Watchdog' -Description 'Audited opt-in MSR/MMIO limit monitor' -StartupType Manual | Out-Null
Write-Host "Installed RaplWatchdog as Manual service. Start it only after read-only validation."
