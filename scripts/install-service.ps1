#Requires -Version 5.1
#Requires -RunAsAdministrator
<#
.SYNOPSIS
Installs the opt-in RaplWatchdog service (manual start, LocalSystem, no automatic restart).

.DESCRIPTION
Registers rapl-watchdog.exe as a manual-start service. It runs as LocalSystem
because the RaplPkg device admits only SYSTEM and Administrators.

Because the service runs as SYSTEM, the script refuses binaries, directories
and an audit log that a non-administrator could modify, replace or redirect
(untrusted owner, write access for other principals, reparse points). New
directories are created with an access list for SYSTEM and Administrators only
(plus read and execute for Users on the program directory). Existing
directories are never re-permissioned; fix them yourself or choose new paths.

The configuration is validated with `rapl-watchdog check-config` before the
service is registered. The service has no recovery actions: after a halt
(service exit code 2) or a configuration error (exit code 1) it stays stopped
until an administrator has reviewed the audit log.

.PARAMETER SourceDir
Optional build output directory (for example target\x86_64-pc-windows-msvc\release).
When given, rapl-watchdog.exe and rapl-bridge.exe are copied into InstallRoot.

.EXAMPLE
.\scripts\install-service.ps1 -SourceDir .\target\x86_64-pc-windows-msvc\release
#>
[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:ProgramFiles 'WinMmioMsr'),
    [string]$SourceDir,
    [string]$AuditLog = (Join-Path $env:ProgramData 'WinMmioMsr\rapl-audit.jsonl'),
    [string]$BackupDir = (Join-Path $env:ProgramData 'WinMmioMsr\backups'),
    [ValidateRange(5, 55)][int]$Pl1 = 55,
    [ValidateRange(5, 157)][int]$Pl2 = 157,
    [ValidateRange(1000, 3600000)][int]$IntervalMs = 5000,
    [ValidateRange(1, 60)][int]$MaxCorrectionsPerHour = 6,
    [ValidateRange(1, 100)][int]$MaxConsecutiveErrors = 3
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ServiceName = 'RaplWatchdog'
$Binaries = @('rapl-watchdog.exe', 'rapl-bridge.exe')
$SystemSid = 'S-1-5-18'
$AdministratorsSid = 'S-1-5-32-544'
$UsersSid = 'S-1-5-32-545'
$TrustedInstallerSid = 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464'
$TrustedOwners = @($SystemSid, $AdministratorsSid, $TrustedInstallerSid)
# CREATOR OWNER and OWNER RIGHTS only ever grant access to the (trusted) owner.
$TrustedWriters = $TrustedOwners + @('S-1-3-0', 'S-1-3-4')
# Write data, append data, write EA, delete child, write attributes, delete,
# write DAC, write owner, generic all, generic write.
$WriteRights = 0x2 -bor 0x4 -bor 0x10 -bor 0x40 -bor 0x100 -bor 0x10000 -bor 0x40000 -bor 0x80000 -bor 0x10000000 -bor 0x40000000
# Rights on an ancestor directory that allow replacing or re-permissioning the path below it.
$AncestorRights = 0x40 -bor 0x10000 -bor 0x40000 -bor 0x80000 -bor 0x10000000
# A volume root cannot be deleted or renamed, so DELETE on it is harmless.
$RootRights = $AncestorRights -band (-bnot 0x10000)

function Resolve-LocalPath([string]$Path, [string]$Name) {
    if ([string]::IsNullOrWhiteSpace($Path) -or $Path.IndexOfAny([char[]]'"*?<>|') -ge 0) {
        throw "$Name is not a valid path: '$Path'"
    }
    if ($Path -notmatch '^[A-Za-z]:\\') {
        throw "$Name must be an absolute local path such as C:\...: '$Path'"
    }
    # A trailing backslash would escape the closing quote in the service command line.
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    if ($full.Length -le 2) { throw "$Name must not be a drive root: '$Path'" }
    $format = (New-Object IO.DriveInfo $full.Substring(0, 1)).DriveFormat
    if ($format -notin @('NTFS', 'ReFS')) {
        throw "$Name must be on an NTFS or ReFS volume (found ${format}): '$full'"
    }
    $full
}

function Get-AccountName([string]$Sid) {
    try { (New-Object Security.Principal.SecurityIdentifier $Sid).Translate([Security.Principal.NTAccount]).Value }
    catch { $Sid }
}

# Refuses a path that a non-administrator could change through ownership,
# an allow entry with any of $Rights, a missing access list or a reparse point.
function Assert-Protected([string]$Path, [int]$Rights) {
    $item = Get-Item -LiteralPath $Path -Force
    if (([int]$item.Attributes -band 0x400) -ne 0) {
        throw "Refusing '$Path': it is a reparse point (link, junction or mount point)."
    }
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($owner -notin $TrustedOwners) {
        throw ("Refusing '{0}': its owner {1} is not SYSTEM, Administrators or TrustedInstaller. " +
            "Fix: icacls `"{0}`" /setowner *{2}") -f $Path, (Get-AccountName $owner), $AdministratorsSid
    }
    if ($acl.GetSecurityDescriptorSddlForm('Access') -match 'NO_ACCESS_CONTROL') {
        throw "Refusing '$Path': it has no access control list."
    }
    foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        $sid = $rule.IdentityReference.Value
        $inheritOnly = ([int]$rule.PropagationFlags -band 2) -ne 0
        if ($rule.AccessControlType -ne 'Allow' -or $inheritOnly -or $sid -in $TrustedWriters) { continue }
        $granted = [int]$rule.FileSystemRights
        if (($granted -band $Rights) -ne 0) {
            throw ("Refusing '{0}': {1} may modify it (rights 0x{2:X8}); only SYSTEM and " +
                "Administrators may have write access.") -f $Path, (Get-AccountName $sid), $granted
        }
    }
}

function Assert-ProtectedAncestry([string]$Path) {
    $current = $Path
    while ($current) {
        $parent = [IO.Path]::GetDirectoryName($current)
        $rights = if ($parent) { $AncestorRights } else { $RootRights }
        Assert-Protected $current $rights
        $current = $parent
    }
}

function Assert-ProtectedTree([string]$Path) {
    Assert-Protected $Path $WriteRights
    $parent = [IO.Path]::GetDirectoryName($Path)
    if ($parent) { Assert-ProtectedAncestry $parent }
}

# Creates missing directories atomically with a protected access list owned by
# Administrators and then verifies the whole chain, which also detects a
# directory planted by someone else between the existence check and creation.
function New-ProtectedDirectory([string]$Path, [switch]$UsersCanRead) {
    $missing = @()
    $cursor = $Path
    while (-not (Test-Path -LiteralPath $cursor)) {
        $missing = @($cursor) + $missing
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    if ($missing) { Assert-ProtectedAncestry $cursor }
    foreach ($dir in $missing) {
        $security = New-Object Security.AccessControl.DirectorySecurity
        $security.SetAccessRuleProtection($true, $false)
        $security.SetOwner((New-Object Security.Principal.SecurityIdentifier $AdministratorsSid))
        $grants = @(@($SystemSid, 'FullControl'), @($AdministratorsSid, 'FullControl'))
        if ($UsersCanRead) { $grants += , @($UsersSid, 'ReadAndExecute') }
        foreach ($grant in $grants) {
            $security.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
                        (New-Object Security.Principal.SecurityIdentifier $grant[0]), $grant[1],
                        'ContainerInherit, ObjectInherit', 'None', 'Allow')))
        }
        if ($PSVersionTable.PSEdition -eq 'Core') {
            [void][IO.FileSystemAclExtensions]::CreateDirectory($security, $dir)
        }
        else {
            [void][IO.Directory]::CreateDirectory($dir, $security)
        }
        Write-Host "Created $dir"
    }
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { throw "'$Path' is not a directory." }
    Assert-ProtectedTree $Path
}

function Invoke-Program([string]$FilePath, [string]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $FilePath
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($info)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $stdout.Result; Error = $stderr }
}

$InstallRoot = Resolve-LocalPath $InstallRoot 'InstallRoot'
$AuditLog = Resolve-LocalPath $AuditLog 'AuditLog'
$BackupDir = Resolve-LocalPath $BackupDir 'BackupDir'
if ($Pl1 -gt $Pl2) { throw "Pl1 ($Pl1 W) must not exceed Pl2 ($Pl2 W)." }
$source = $null
if ($SourceDir) {
    $source = Resolve-LocalPath $SourceDir 'SourceDir'
    foreach ($name in $Binaries) {
        $from = Join-Path $source $name
        if (-not (Test-Path -LiteralPath $from -PathType Leaf)) { throw "Missing $from" }
    }
    New-ProtectedDirectory $InstallRoot -UsersCanRead
}
elseif (Test-Path -LiteralPath $InstallRoot -PathType Container) {
    Assert-ProtectedTree $InstallRoot
}
else {
    throw "InstallRoot '$InstallRoot' does not exist; pass -SourceDir to deploy the binaries."
}

New-ProtectedDirectory ([IO.Path]::GetDirectoryName($AuditLog))
New-ProtectedDirectory $BackupDir
if (Test-Path -LiteralPath $AuditLog) {
    if (-not (Test-Path -LiteralPath $AuditLog -PathType Leaf)) { throw "AuditLog '$AuditLog' is not a file." }
    Assert-Protected $AuditLog $WriteRights
}

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing -and $existing.Status -ne 'Stopped') {
    Write-Host "Stopping $ServiceName ..."
    Stop-Service -Name $ServiceName
}

$watchdog = Join-Path $InstallRoot 'rapl-watchdog.exe'
$bridge = Join-Path $InstallRoot 'rapl-bridge.exe'
if ($source) {
    foreach ($name in $Binaries) {
        $target = Join-Path $InstallRoot $name
        Copy-Item -LiteralPath (Join-Path $source $name) -Destination $target -Force
        $acl = Get-Acl -LiteralPath $target
        $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier $AdministratorsSid))
        Set-Acl -LiteralPath $target -AclObject $acl
        Write-Host "Deployed $target"
    }
}
foreach ($binary in @($watchdog, $bridge)) {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Missing $binary" }
    Assert-Protected $binary $WriteRights
}

$options = ('--bridge "{0}" --audit-log "{1}" --backup-dir "{2}" --pl1 {3} --pl2 {4} --interval-ms {5} ' +
    '--max-corrections-per-hour {6} --max-consecutive-errors {7}') -f $bridge, $AuditLog, $BackupDir,
$Pl1, $Pl2, $IntervalMs, $MaxCorrectionsPerHour, $MaxConsecutiveErrors
$check = Invoke-Program $watchdog "check-config $options"
if ($check.ExitCode -ne 0) {
    throw "rapl-watchdog check-config failed with exit code $($check.ExitCode): $($check.Error.Trim())"
}
Write-Host 'Validated configuration:'
Write-Host $check.Output.TrimEnd()

if ($existing) {
    $existing.Close()
    & sc.exe delete $ServiceName | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe delete $ServiceName failed with exit code $LASTEXITCODE." }
    $deadline = (Get-Date).AddSeconds(30)
    while (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        if ((Get-Date) -gt $deadline) {
            throw "$ServiceName is still marked for deletion; close the Services console or reboot, then retry."
        }
        Start-Sleep -Milliseconds 250
    }
}

$imagePath = "`"$watchdog`" run $options"
$description = 'Re-applies audited RAPL package power limits within a correction budget (opt-in, manual start).'
New-Service -Name $ServiceName -BinaryPathName $imagePath -DisplayName 'RAPL Limit Watchdog' `
    -Description $description -StartupType Manual | Out-Null
$service = Get-CimInstance -ClassName Win32_Service -Filter "Name='$ServiceName'"
if ($service.PathName -ne $imagePath -or $service.StartMode -ne 'Manual' -or $service.StartName -ne 'LocalSystem') {
    throw "$ServiceName was registered with unexpected settings: $($service.PathName) / $($service.StartMode) / $($service.StartName)"
}

Write-Host ''
Write-Host "Installed $ServiceName (manual start, LocalSystem, no automatic restart)."
Write-Host "Start only after read-only validation on this machine: sc.exe start $ServiceName"
Write-Host "Stop: Stop-Service $ServiceName"
Write-Host "Audit: win-mmio-msr-cli audit `"$AuditLog`""
Write-Host 'Service exit code 1 = configuration error, 2 = halted (correction budget or consecutive errors).'
