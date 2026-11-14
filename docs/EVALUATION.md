# Evaluation on the target device

Owner-approved evaluation with a test-signed driver, because no Microsoft attestation signing is
available. This page is the restart point for a new agent session or a reinstalled machine. Open
items are in [TODO.md](TODO.md); the full report structure is in
[VALIDATION_REPORT_TEMPLATE.md](VALIDATION_REPORT_TEMPLATE.md).

## Target device (recorded 2026-11-14, before a planned reinstall)

| Item | Value |
|---|---|
| Device | Lenovo machine type 21FB (ThinkPad P16 Gen 2 class) |
| CPU | 13th Gen Intel Core i9-13950HX (Raptor Lake HX) |
| BIOS / EC | N3TET64W (2.00) / 1.58 |
| Windows | 11 Enterprise, build 26200 |
| Host bridge | PCI `VEN_8086 DEV_A702`, revision 01 |
| Secure Boot | off |
| Test signing | off before the evaluation |
| VBS / HVCI | running / on (security services 1, 2, 7) |
| Vulnerable driver blocklist | enabled |
| Management | MDM-managed, several active WDAC / Code Integrity policies |
| Power tools | Intel Innovation Platform Framework (`ipfsvc`) and the vendor power utility were running; both may override PL1/PL2 |

Re-record all values after a reinstall: Secure Boot, HVCI, WDAC policies and power tools may
differ. Test signing only takes effect while Secure Boot is off. Never disable Secure Boot, HVCI or
WDAC for this project; if a policy blocks the driver, record it and stop.

## Status

| Step | State |
|---|---|
| Default read-only driver from CI (`main` at `9b8cefd`, run 43554234365), all gates off | built |
| Test-signed, service `rapl-pkg` registered, test signing on | done 2026-11-14, **lost by the reinstall; repeat** |
| Reboot, `sc.exe start rapl-pkg` | not done |
| Read-only `status` (MSR `0x606`, `0x610`) and comparison with a reference tool | not done |
| MMIO read, layout validation, writes, watchdog, thermal run | not done |

Decision so far: no hardware evidence; nothing is validated.

## Restart after a reinstall

Run in an elevated PowerShell (for example through `gsudo`) from the repository root.

1. Get the build artifacts (or newer ones from a green `main` run):

   ```powershell
   gh run download 43554234365 --repo thomasdenk79-cyber/win-mmio-msr-cli `
       --name driver-package-x64-unsigned --name user-mode-x64 --dir .\eval
   ```

   CI artifacts expire; if the download fails, start the `ci` workflow on `main` again and use
   the new run ID (`gh run list --workflow ci --branch main`).

2. Record the system state (section above) and check `Confirm-SecureBootUEFI`.
3. Test-sign and register:

   ```powershell
   .\scripts\eval-testsign.ps1 -DriverDir .\eval\driver-package-x64-unsigned\rapl-pkg -UserModeDir .\eval\user-mode-x64
   ```

4. Reboot.
5. Start and read only:

   ```powershell
   sc.exe start rapl-pkg
   sc.exe query rapl-pkg
   $eval = Join-Path $env:ProgramFiles 'WinMmioMsrEval'
   & "$eval\win-mmio-msr-cli.exe" status --bridge "$eval\rapl-bridge.exe"
   & "$eval\win-mmio-msr-cli.exe" status --bridge "$eval\rapl-bridge.exe" --json
   ```

   If the start fails, read the Code Integrity log and record event IDs and policy names:

   ```powershell
   Get-WinEvent -LogName 'Microsoft-Windows-CodeIntegrity/Operational' -MaxEvents 30 |
       Where-Object Id -In 3033, 3077, 3089 | Format-List TimeCreated, Id, Message
   ```

6. Continue with TODO E4 onward. Every write step needs a gate build from CI, `--backup` and
   `--audit-log`, and an independent review of the read-only evidence first.
7. When the evaluation ends: `.\scripts\eval-rollback.ps1`, then reboot.

## Notes for agents

- From a non-elevated agent shell, `gsudo` output does not come back reliably. Write the elevated
  commands to a script that logs to a file with a final `DONE` line, start it with
  `Start-Process gsudo -ArgumentList powershell, -NoProfile, -File, <script> -WindowStyle Hidden`,
  and poll the log file. The owner confirms the UAC prompt.
- Ask the owner to reboot; never reboot unattended.
- Keep serial numbers, user names and certificate private keys out of this repository.

## Comparison: how ThrottleStop accesses the hardware

ThrottleStop ships its own Microsoft-signed kernel driver (`ThrottleStop.sys`). Older versions
expose generic physical memory and MSR access to user mode (CVE-2025-7771, used in attacks to
disable security software), which is why such drivers end up on the vulnerable driver blocklist.
`rapl-pkg` deliberately allows only MSR `0x606`/`0x610` and the single MCHBAR power limit
register, restricted to SYSTEM and Administrators, so that it can be signed and kept off the
blocklist.
