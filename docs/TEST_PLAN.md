# Test plan

## Automated (CI, every pull request)

| Job | Coverage |
|---|---|
| `rust-linux` | `cargo fmt --check`, Clippy with `-D warnings`, all unit tests, release build |
| `rust-windows` | Clippy for the Windows code paths, all unit tests, MSVC release build, [`scripts/smoke.ps1`](../scripts/smoke.ps1), [`.github/scripts/service-smoke.ps1`](../.github/scripts/service-smoke.ps1) |
| `powershell-lint` | Windows PowerShell 5.1 parser and PSScriptAnalyzer for every script |
| `driver-windows` | WDK build with warnings as errors and ApiValidator, compile check with every gate enabled, code analysis (Driver Recommended Rules), InfVerif and package check |

The unit tests cover the register encoding, the policy, the plan and apply sequence including the
race check and the rollback, the audit format and the watt columns, the bridge command whitelist,
gates, policy refusals and compare-and-write handling with a fake driver, the info protocol, the
driver header against the Rust ABI and the watchdog configuration, correction budget and halt
rules.

`scripts/smoke.ps1` runs the release CLI in simulation: `status`, `plan` and `apply` with the
PL1/PL2 before and set watt columns in JSON and CSV, the backup, a no-op apply and the refusal of
unsafe requests. `service-smoke.ps1` installs the watchdog with the real Service Control Manager
(without a driver) and checks the installer refusals, the refused start parameters, the halt after
consecutive bridge errors and the audit records.

## Hardware phases

Each phase needs the previous one to pass. Record the results in the
[validation report](VALIDATION_REPORT_TEMPLATE.md).

| Phase | Build | Test | Expected result |
|---|---|---|---|
| H1 | none | System identity, Secure Boot, HVCI, blocklist | Values recorded; no security feature disabled |
| H2 | default, signed | Install; `rapl-bridge info`; `status` | Bridge reports protocol 2 and the CPU gate; MSR values match HWiNFO and ThrottleStop three times |
| H3 | `RaplEnableMmioRead=1`, signed | MCHBAR evidence; `status` | MCHBAR enabled and base plausible; MMIO equals MSR or the difference is explained |
| H4 | H3 | Bridge error paths: missing device, non-administrator, unsupported register | Clear error; no register access |
| H5 | all gates, signed, after review | `plan`, `apply` with a small PL1 change | Audit records with PL1/PL2 before, set and readback watts; backup; exact readback |
| H6 | H5 | Same limits again | `changed: false`; no backup and no write |
| H7 | H5 | Firmware or tool changes a register between `plan` and `apply` | Apply aborts without writing, or the driver returns `STATUS_RETRY` |
| H8 | H5 | Forced readback mismatch or bridge failure | Rollback records with `phase: rollback`; the restored values are read back |
| H9 | H5 | Watchdog with `-MaxCorrectionsPerHour 2` and firmware resets | Corrections audited; the service halts with exit code 2 |
| H10 | H5 | Sleep and resume, BIOS profile change, thermal load | Behaviour documented; no blind re-apply loop; no WHEA errors |

## Stop criteria

Unverified MCHBAR base or layout, unsigned or blocked driver, any required security feature
disablement, inconsistent reads, unexpected firmware changes, thermal or EC alarms, WHEA errors or
a failed rollback.
