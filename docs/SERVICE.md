# Watchdog service

`rapl-watchdog.exe` is an opt-in Windows service (`RaplWatchdog`). It periodically compares MSR
`0x610` and `MCHBAR+0x59A0` with the configured target through the bridge and re-applies the
target when firmware, the embedded controller or another tool changed it. It is installed with
manual start, runs as LocalSystem and has no recovery actions.

Install it only on a machine with a signed driver build that has live writes enabled and a
completed [validation report](VALIDATION_REPORT_TEMPLATE.md).

## Behaviour

Every interval the watchdog starts the bridge, reads both registers and builds the same plan as
`win-mmio-msr-cli plan`:

- Registers already at the target: nothing is written and nothing is logged.
- Bridge, plan, apply or audit errors: a `service_error` record; after
  `--max-consecutive-errors` errors in a row the service halts.
- Otherwise it reserves one correction from the rolling one-hour budget, logs
  `correction_detected` and applies the plan through the normal apply path: backup
  (`rapl-before-<UTC time>-<sequence>.txt`), race check, MMIO then MSR, readback and rollback.
  It then logs `correction_verified`. When the budget is exhausted the service halts instead of
  fighting the firmware. Only the newest 100 watchdog backups are kept.

PL1 and PL2 are free parameters within `5 <= PL1 <= PL2 <= 157 W`. The service halts rather than
retries
forever: a halted service stays stopped until an administrator has reviewed the audit log and
starts it again.

## Configuration

The configuration is part of the service command line (ImagePath), so it is visible and validated
at installation:

```text
"C:\Program Files\WinMmioMsr\rapl-watchdog.exe" run --bridge <rapl-bridge.exe> --audit-log <file> --backup-dir <dir> [options]
```

| Option | Default | Range |
|---|---|---|
| `--bridge`, `--audit-log`, `--backup-dir` | required | absolute paths |
| `--pl1` | 55 | 5 to 157 W, at most `--pl2` |
| `--pl2` | 157 | 5 to 157 W |
| `--interval-ms` | 5000 | 1000 to 3600000 |
| `--max-corrections-per-hour` | 6 | 1 to 60 |
| `--max-consecutive-errors` | 3 | 1 to 100 |

`rapl-watchdog.exe check-config <options>` validates the options and prints the resulting
configuration without hardware access. Start parameters (`sc.exe start RaplWatchdog <args>`) are
refused and audited as `service_start_refused`.

## Exit codes and audit events

| Service-specific exit code | Meaning |
|---|---|
| 1 | Configuration error or refused start parameters |
| 2 | Halted: correction budget exhausted or too many consecutive errors |

| Event | Content |
|---|---|
| `service_started` | `version`, `pl1_target_w`, `pl2_target_w`, `interval_ms`, `max_corrections_per_hour`, `max_consecutive_errors` |
| `correction_detected` | One record per register: PL1/PL2 before and set watts, raw values, `backup` |
| `correction_verified` | As above plus the readback watts after the apply |
| `write_attempt`, `write_result`, ... | Written by the bridge for every register write (see [BRIDGE_PROTOCOL.md](BRIDGE_PROTOCOL.md)) |
| `service_error` | `error`, `consecutive_errors` |
| `backup_prune_failed` | `error` |
| `service_halted` | `halt_reason` |
| `service_stopped` | Normal stop |

```powershell
& 'C:\Program Files\WinMmioMsr\win-mmio-msr-cli.exe' audit "$env:ProgramData\WinMmioMsr\rapl-audit.jsonl" --last 20
Get-Content -LiteralPath "$env:ProgramData\WinMmioMsr\rapl-audit.jsonl" -Wait -Tail 20
```

## Install

From an elevated Windows PowerShell 5.1 or PowerShell 7 prompt in the repository:

```powershell
.\scripts\install-service.ps1 -SourceDir .\target\x86_64-pc-windows-msvc\release
```

| Parameter | Default |
|---|---|
| `-InstallRoot` | `%ProgramFiles%\WinMmioMsr` |
| `-SourceDir` | none; when given, `rapl-watchdog.exe` and `rapl-bridge.exe` are copied into `InstallRoot` |
| `-AuditLog` | `%ProgramData%\WinMmioMsr\rapl-audit.jsonl` |
| `-BackupDir` | `%ProgramData%\WinMmioMsr\backups` |
| `-Pl1`, `-Pl2` | 55, 157 |
| `-IntervalMs`, `-MaxCorrectionsPerHour`, `-MaxConsecutiveErrors` | 5000, 6, 3 |

Because the service runs as LocalSystem, the installer refuses binaries, directories (including
their parents) and an audit log that a non-administrator could modify, replace or redirect:
reparse points, owners other than SYSTEM, Administrators or TrustedInstaller, and write access for
other principals. It creates missing directories with an access list for SYSTEM and Administrators
only (the program directory also grants Users read and execute) and never changes the permissions
of existing directories. It validates the configuration with `check-config`, replaces an existing
`RaplWatchdog` registration and verifies the registered command line, start type and account.

`win-mmio-msr-cli.exe` is not copied by the installer; copy it next to the other binaries if
you want to view the audit log from the install directory.

## Operate and remove

```powershell
sc.exe start RaplWatchdog
Get-Service RaplWatchdog
sc.exe query RaplWatchdog            # SERVICE_EXIT_CODE shows 1 or 2 after a failure
Stop-Service RaplWatchdog

# remove
Stop-Service RaplWatchdog -ErrorAction SilentlyContinue
sc.exe delete RaplWatchdog
Remove-Item -LiteralPath "$env:ProgramFiles\WinMmioMsr" -Recurse   # optional
```

Keep the audit log and the backups as evidence; do not edit the original file. Hand over
read-only copies.
