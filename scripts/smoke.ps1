#Requires -Version 5.1
<#
.SYNOPSIS
End-to-end smoke test of win-mmio-msr-cli with the simulation backend.

.DESCRIPTION
Runs status, plan, apply and the audit viewer with --mock and checks the JSON
and CSV output, including the PL1/PL2 before and set watt columns and the
MMIO-then-MSR write order. It also checks that an apply of the current limits
writes nothing and that the CLI refuses unsafe requests: a PL1 above the
processor base power without consent, PL1 above PL2, an apply without --yes,
an existing backup file and --csv together with --json.

All files go to a new temporary folder that is removed afterwards. The test
needs no driver, no administrator rights and no hardware access.

.PARAMETER BinaryDir
Folder that contains win-mmio-msr-cli.exe.

.EXAMPLE
.\scripts\smoke.ps1

.EXAMPLE
.\scripts\smoke.ps1 -BinaryDir .\target\release
#>
[CmdletBinding()]
param(
    [string]$BinaryDir = (Join-Path $PSScriptRoot '..\target\x86_64-pc-windows-msvc\release')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Cli = Join-Path $BinaryDir 'win-mmio-msr-cli.exe'
if (-not (Test-Path -LiteralPath $Cli -PathType Leaf)) {
    throw "$Cli not found. Run scripts\build.ps1 first or pass -BinaryDir."
}

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "Assertion failed: $Message" }
}

# Returns the stdout lines of one CLI run; stderr passes through to the console.
function Invoke-Cli([string[]]$Arguments) {
    $lines = & $Cli @Arguments
    if ($LASTEXITCODE -ne 0) { throw "win-mmio-msr-cli $($Arguments -join ' ') failed with exit code $LASTEXITCODE." }
    $lines
}

function Assert-Refused([string]$Name, [string[]]$Arguments) {
    # Expected error output on stderr must not stop the script.
    $ErrorActionPreference = 'Continue'
    $lines = & $Cli @Arguments 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message.Trim() } else { "$_".Trim() }
    }
    $code = $LASTEXITCODE
    $message = @($lines | Where-Object { $_ }) -join ' '
    Assert-That ($code -ne 0) "$Name was accepted"
    Write-Host "ok  refused: $Name (exit code $code): $message"
}

$Work = Join-Path ([IO.Path]::GetTempPath()) ('win-mmio-msr-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Work | Out-Null
try {
    Write-Host '--- 1. status reads the simulated registers'
    $status = Invoke-Cli @('--json', 'status', '--mock') | ConvertFrom-Json
    Assert-That ($status.schema_version -eq 2 -and $status.ok -and $status.command -eq 'status') 'unexpected JSON envelope'
    Assert-That ($status.snapshot.identity.simulated -eq $true) 'status is not marked as simulated'
    Assert-That ($status.snapshot.synchronized -eq $true) 'simulated MSR and MMIO differ'
    Invoke-Cli @('status', '--mock') | Out-Host

    Write-Host '--- 2. plan shows PL1/PL2 before and set for each register path'
    $plan = (Invoke-Cli @('--json', 'plan', '--mock', '--pl1', '50', '--pl2', '120') | ConvertFrom-Json).plan
    $changes = @($plan.changes)
    Assert-That ($changes.Count -eq 2) "expected 2 register changes, got $($changes.Count)"
    foreach ($change in $changes) {
        Assert-That ($change.pl1_before_w -eq 55 -and $change.pl2_before_w -eq 157) "$($change.register): before $($change.pl1_before_w)/$($change.pl2_before_w) W"
        Assert-That ($change.pl1_set_w -eq 50 -and $change.pl2_set_w -eq 120) "$($change.register): set $($change.pl1_set_w)/$($change.pl2_set_w) W"
    }
    Assert-That (@($plan.blockers).Count -eq 0) "plan is blocked: $(@($plan.blockers) -join '; ')"
    Invoke-Cli @('plan', '--mock', '--pl1', '50', '--pl2', '120') | Out-Host

    Write-Host '--- 3. apply writes both registers with backup, readback and audit log'
    $backup = Join-Path $Work 'rapl-before.txt'
    $log = Join-Path $Work 'rapl-audit.jsonl'
    $applied = Invoke-Cli @('--json', 'apply', '--mock', '--pl1', '50', '--pl2', '120', '--yes',
        '--backup', $backup, '--audit-log', $log, '--reason', 'smoke_test') | ConvertFrom-Json
    Assert-That ($applied.changed -eq $true) 'apply reported no change'
    Assert-That ($applied.after.synchronized -eq $true) 'MSR and MMIO differ after apply'
    Assert-That ((Get-Content -LiteralPath $backup -TotalCount 1) -eq 'WIN_MMIO_MSR_BACKUP_V2') 'backup header missing'
    $backupHash = (Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash

    Write-Host '--- 4. the audit log holds the watt columns of every register change'
    Invoke-Cli @('audit', $log) | Out-Host
    $rows = @(Invoke-Cli @('audit', $log, '--csv') | ConvertFrom-Csv)
    Assert-That ($rows.Count -eq 4) "expected 4 audit records, got $($rows.Count)"
    $sequence = @($rows | ForEach-Object { "$($_.event) $($_.register)" }) -join ', '
    $expected = 'write_attempt MMIO_MCHBAR_0x59A0, write_result MMIO_MCHBAR_0x59A0, write_attempt MSR_0x610, write_result MSR_0x610'
    Assert-That ($sequence -eq $expected) "unexpected write sequence: $sequence"
    foreach ($row in $rows) {
        $where = "line $($row.line) ($($row.event) $($row.register))"
        Assert-That ($row.pl1_before_w -eq '55' -and $row.pl2_before_w -eq '157') "${where}: before $($row.pl1_before_w)/$($row.pl2_before_w) W"
        Assert-That ($row.pl1_set_w -eq '50' -and $row.pl2_set_w -eq '120') "${where}: set $($row.pl1_set_w)/$($row.pl2_set_w) W"
        Assert-That ($row.phase -eq 'apply' -and $row.reason -eq 'smoke_test' -and $row.simulated -eq 'true') "${where}: phase, reason or simulated flag"
        if ($row.event -eq 'write_result') {
            Assert-That ($row.outcome -eq 'changed' -and $row.pl1_readback_w -eq '50' -and $row.pl2_readback_w -eq '120') "${where}: readback $($row.pl1_readback_w)/$($row.pl2_readback_w) W, outcome $($row.outcome)"
        }
    }

    Write-Host '--- 5. apply of the current limits writes nothing'
    $noop = Join-Path $Work 'no-op-backup.txt'
    $result = Invoke-Cli @('--json', 'apply', '--mock', '--preset', 'base', '--yes', '--backup', $noop) | ConvertFrom-Json
    Assert-That ($result.changed -eq $false) 'apply of the current limits changed registers'
    Assert-That (-not (Test-Path -LiteralPath $noop)) 'apply of the current limits created a backup'

    Write-Host '--- 6. unsafe requests are refused'
    Assert-Refused 'PL1 above 55 W without --allow-sustained-above-base' @('plan', '--mock', '--pl1', '60', '--pl2', '157')
    Assert-Refused 'PL1 above PL2' @('plan', '--mock', '--pl1', '50', '--pl2', '40')
    Assert-Refused 'apply without --yes' @('apply', '--mock', '--pl1', '50', '--pl2', '120', '--backup', (Join-Path $Work 'unused.txt'))
    Assert-Refused 'apply onto an existing backup file' @('apply', '--mock', '--pl1', '50', '--pl2', '120', '--yes', '--backup', $backup)
    Assert-Refused '--csv together with --json' @('--json', 'audit', $log, '--csv')
    Assert-That ((Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash -eq $backupHash) 'the existing backup was modified'
    Assert-That (-not (Test-Path -LiteralPath (Join-Path $Work 'unused.txt'))) 'a refused apply created a backup'
}
finally {
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host 'Smoke test passed. No hardware was accessed.'
