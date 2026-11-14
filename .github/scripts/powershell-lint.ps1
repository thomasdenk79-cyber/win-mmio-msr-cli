#Requires -Version 5.1
<#
.SYNOPSIS
Parses and analyzes every PowerShell file of the repository.

.DESCRIPTION
Fails when a .ps1, .psm1 or .psd1 file does not parse in the running PowerShell
version or when PSScriptAnalyzer reports a finding with the repository settings
(PSScriptAnalyzerSettings.psd1). The folders .git and target are skipped.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$settings = Join-Path $root 'PSScriptAnalyzerSettings.psd1'
$extensions = @('.ps1', '.psm1', '.psd1')
$skipped = @('.git', 'target')

$files = @(
    @(
        Get-ChildItem -LiteralPath $root -File -Force
        Get-ChildItem -LiteralPath $root -Directory -Force | Where-Object { $skipped -notcontains $_.Name } |
            Get-ChildItem -Recurse -File -Force
    ) | Where-Object { $extensions -contains $_.Extension }
)
if ($files.Count -eq 0) { throw 'No PowerShell files found.' }

$parseErrors = 0
foreach ($file in $files) {
    $tokens = $null
    $errors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)
    foreach ($problem in @($errors)) {
        Write-Host "$($file.FullName):$($problem.Extent.StartLineNumber): $($problem.Message)"
        $parseErrors++
    }
}

Import-Module PSScriptAnalyzer
$findings = @($files | ForEach-Object { Invoke-ScriptAnalyzer -Path $_.FullName -Settings $settings })
if ($findings.Count -gt 0) {
    $findings | Format-Table -AutoSize -Wrap ScriptName, Line, Severity, RuleName, Message | Out-String -Width 240 | Write-Host
}
Write-Host "PSScriptAnalyzer $((Get-Module PSScriptAnalyzer).Version), PowerShell $($PSVersionTable.PSVersion): $($files.Count) files"
if ($parseErrors -gt 0 -or $findings.Count -gt 0) {
    throw "$parseErrors parse error(s) and $($findings.Count) analyzer finding(s)."
}
Write-Host 'No parse errors and no analyzer findings.'
