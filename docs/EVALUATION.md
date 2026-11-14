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

## Attestation signing: a registered business changes the signing route

Earlier parts of this document assumed Microsoft attestation signing was "not available" because
it requires an organization. The owner has a registered trade (`Gewerbeanmeldung`) for a solar PV
installation (being below the Berufsgenossenschaft membership threshold does not invalidate the
registration itself), separate from unrelated, non-trade-registered agricultural income. A
registered sole-proprietor trade is sufficient for Microsoft's Hardware Developer Program; it does
not need to relate to software and does not need a Handelsregister entry.

What this unlocks: attestation signing needs only (1) a free Microsoft Entra ID tenant with the
owner as global administrator, (2) the owner as legal contact, and (3) an EV code signing
certificate issued in the exact legal/trade name from the `Gewerbeanmeldung` (around EUR 330-360
per year, for example via Certum; requires the trade registration extract, a tax-office
confirmation, photo ID and a callback verification). Registration with Microsoft itself and
attestation signing per submission are free. Once approved, the signed driver needs no permanent
test-signing mode and no "Test Mode" desktop watermark, and loads normally even with HVCI on.

This is the preferred long-term signing route over the evaluation test-signing script; it replaces
TODO item R1. Not started: still need the trade registration document, the EV certificate order,
and the Partner Center registration.

Open legal nuance: the owner's farm has no `Gewerbeschein` because a trade serving agriculture as
an ancillary operation (for example horse boarding) does not require separate trade registration
under German law and is counted within the agricultural income on the tax assessment
(`landwirtschaftliche Einkünfte`). The solar PV installation runs as its own trade with its own tax
number, which is the stronger document for a CA, but whether a tax-office confirmation without a
formal `Gewerbeschein` certificate is sufficient for EV validation is CA-specific and unverified
here; ask the chosen CA's support directly with the actual documents before paying.

The owner is in the process of dissolving that trade. This does not block the plan: an EV
certificate and an active trade registration are only needed at the time of Partner Center
registration and the one-time attestation-signing submission. Once Microsoft has attestation-signed
and timestamped the driver, that signature keeps working indefinitely even after the certificate
expires or is revoked, the trade is dissolved, or the Partner Center account is closed; Windows
checks the timestamped Microsoft counter-signature, not the owner's ongoing business status. Only
submitting a new/updated driver afterward would need an active registration again. So the practical
sequence is: buy a 1-year EV certificate, register, submit and get this driver signed once, then
dissolve the trade as planned.

Simplest practical option, avoiding the agriculture/PV documentation question entirely: register a
new, small trade in the owner's own name for a plausible purpose (for example "Softwareentwicklung"
/ "IT-Dienstleistungen", fitting this project) with the local `Gewerbeamt`. This is a routine, legal
step even if the owner later decides the business idea is not worth pursuing and deregisters it
again after a few months; there is no bad faith as long as the registration was made with genuine
intent to operate at the time, which is enough for a `Gewerbeschein`. Cost is typically EUR 20-60
one-time, processed same-day, and deregistration later is similarly simple and cheap. This yields a
plain `Gewerbeschein` in the owner's name, the most unambiguously accepted document for EV
validation, side-stepping the CA-specific uncertainty around tax-only agricultural documentation.
Order of operations: register this trade -> obtain the EV certificate using the `Gewerbeschein` ->
register with Microsoft Partner Center and submit this driver for attestation signing -> deregister
the trade afterward; the signed driver keeps working per the point above.
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
blocklist. Reusing `ThrottleStop.sys`'s undocumented IOCTL interface to avoid signing this
project's own driver is explicitly out of scope: it would reintroduce the same arbitrary
physical-memory primitive as a dependency and turn this project into a vector for it.

## Alternative without test signing: Intel XTU

[Intel Extreme Tuning Utility](https://www.intel.com/content/www/us/en/download/17881/)
ships a Microsoft-signed driver and a command-line tool, `XtuCli.exe` (for example
`XtuCli.exe -t PL1 -v 90`), so it needs no test signing, no reboot for a signature change and no
owner-approved evaluation exception. It is Intel's own, actively maintained tool, which this
project is not.

Caveats specific to an OEM laptop such as this device:
- XTU targets unlocked desktop CPUs on an overclocking-capable chipset; HX mobile CPUs are listed
  as supported but the laptop vendor's BIOS/EC may lock or silently revert the limits XTU sets,
  the same behavior this project's own TODO (E5) already flags for Intel IPF and vendor power
  tools.

**Tested on this device (2026-11-14):** downloaded `XTUSetup_7.14.2.135.exe` directly from
`downloadmirror.intel.com` (SHA-512 matched Intel's download page exactly) and ran a silent
install (`/quiet /norestart`, exit code 3010 = success, reboot pending). Result: **this build
ships no command-line tool at all**, only `XtuUiLauncher.exe` and `PerfTune.exe` (both GUI). The
`XtuCli.exe -t PL1 -v <watt>` syntax referenced in earlier research for this document does not
exist in the current release and should be treated as wrong. Since this project's requirement is
a scriptable CLI, XTU does not meet it, and was uninstalled again (`/uninstall /quiet /norestart`,
exit code 0, no leftover files) without rebooting. Conclusion: stay with this repository's own
driver and CLI; XTU is not a substitute here.
