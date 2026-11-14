# Architecture and code review

Review of `main` before branch `feat/watt-columns-architecture-review` (baseline) and the fixes
made on that branch. Every finding was verified against the baseline source; claims that could
not be confirmed were dropped.

## Scope and method

- Read every source file: `src/`, `bridge/`, `service/`, `driver/`, `scripts/`, `.github/`,
  `docs/`.
- Traced the data flow CLI → bridge → driver → hardware and back, including the audit path.
- Compared the driver with the KMDF contracts (`EvtIoDeviceControl` signature, METHOD_BUFFERED
  buffer sharing, non-PnP control devices) and the INF with the documented service install.
- Checked the service against the SCM contract (start parameters, status transitions, exit codes).
- Ran the new CI jobs: Rust on Linux and Windows, MSVC build, CLI and service smoke tests,
  PSScriptAnalyzer, KMDF build with code analysis (DriverRecommendedRules) and package check.

## Verdict

The baseline could not change a power limit on real hardware: the driver created no device, and
the service panicked on every start. The policy existed only in the CLI, and auditing was
optional. All findings below are fixed on the branch. Hardware readiness is **not** claimed:
driver signing, MCHBAR layout validation and the hardware test plan are still open.

## Findings

| ID | Severity | Area | Finding | Resolution | Commit |
|---|---|---|---|---|---|
| C1 | Critical | Driver | `WDF_DRIVER_CONFIG_INIT` registered a PnP `EvtDeviceAdd`, but the INF installs a plain service, so no device object was ever created. | Non-PnP control device created in `DriverEntry`, `EvtDriverUnload` deletes it. | `c97de7b` |
| C2 | Critical | Driver | `RaplEvtIoDeviceControl` declared input and output lengths in the wrong order (KMDF passes output first). | Correct signature; lengths validated with exact sizes. | `c97de7b` |
| C3 | Critical | Driver | The response was zeroed in the shared METHOD_BUFFERED buffer before the request field was read. | Request copied to a local first, response built afterwards. | `c97de7b` |
| C4 | Critical | Driver, bridge | SDDL `D:P(A;;GA;;;SY)(A;;GR;;;BA)` gave Administrators read access only, while the bridge always opened the device read-write, so every call by an administrator failed. | SDDL grants GA to SY and BA; reads open a read-only handle. | `c97de7b`, `470512d` |
| C5 | Critical | Service | `service_main` parsed the SCM start parameters (the first is the service name), failed with "unknown service argument" and panicked. | Configuration comes only from ImagePath; start parameters are refused with exit code 1. | `c6acd4b` |
| C6 | Critical | Installer | No admin check, user-chosen install root, inherited ProgramData ACLs (Users may create files), no ownership or reparse-point checks: a local user could plant binaries that run as LocalSystem or tamper with the audit log. | Elevation required; protected ACLs; ownership, reparse and writability checks; `check-config` before registration. | `c6acd4b` |
| H1 | High | CLI, audit | No readable PL1/PL2 before and set watts; audit records had no schema version or phase. | Four watt columns in plan, audit table, JSON and CSV; audit schema 2 with `phase`. | `67fc819` |
| H2 | High | Bridge, CLI | `RAPL_AUDIT_LOG` was optional: without it the bridge wrote unaudited, and a live apply did not require `--audit-log`. | Bridge refuses writes without an audit log; live `apply` requires `--audit-log`. | `67fc819`, `470512d` |
| H3 | High | Policy | Limits lived only in the CLI and were duplicated in the service; the bridge accepted any value; the driver checked only lock, mask and expected value. | `rapl-core::policy` is the single source; the bridge and the driver (`RaplWriteAllowed`) enforce the same register rule. | `470512d`, `c97de7b`, `c6acd4b` |
| H4 | High | Service | No correction budget and no error limit (endless fight with firmware); `Running` reported before initialisation; always exited with Win32 0. | Hourly correction budget, consecutive-error limit, correct SCM states and service-specific exit codes. | `c6acd4b` |
| H5 | High | Driver | The CPU check only filled the info structure and was not enforced on reads or writes; vendor and package count were hard-coded. | CPU gate enforced on every read and write; vendor from CPUID, package count from the processor relationship information. | `c97de7b` |
| H6 | High | Build | Skeleton INF without SourceDisksNames, SourceDisksFiles, DestinationDirs or PnpLockdown; a CMake file suggested a non-KMDF build. | Complete INF, MSBuild project, CI driver build with package check; CMake file removed. | `c97de7b` |
| M1 | Medium | Build | No Cargo workspace, three lock files, unformatted code. | Workspace with shared `rapl-core`, one lock file, `cargo fmt --check` in CI. | `316bf53` |
| M2 | Medium | CLI | `status` required verified MMIO and failed on systems where only the MSR was readable. | `status` works without MMIO; `apply` reports blockers instead. | `67fc819` |
| M3 | Medium | CLI | A no-op `apply` still created a backup and issued both writes. | No write and no backup when the registers already hold the target. | `67fc819` |
| M4 | Medium | Audit | Rollback writes could not be distinguished from apply writes. | `phase=rollback` on rollback records. | `67fc819` |
| M5 | Medium | Protocol | Identity matched by model-name substring; protocol 1 could not report MMIO readability or the MCHBAR base; lenient size checks and no ABI asserts. | Protocol 2 with family/model/stepping, gate flags and MCHBAR; exact sizes; `C_ASSERT` and a Rust test that parses the driver header. | `470512d`, `c97de7b` |
| M6 | Medium | CI | No fmt check, no Windows or bridge/service clippy, no driver build, no script lint, no smoke tests, Node 20 actions, default token permissions. | Four CI jobs with read-only token permissions and Node 24 actions. | `316bf53`, `c97de7b`, `7464ca7`, `ab60585` |
| M7 | Medium | Scripts | German messages, no exit-code checks, duplicate `build-windows.ps1`. | English scripts with strict exit-code checks; duplicate removed. | `ab60585` |
| M8 | Medium | Docs | README used a nonexistent `status --bridge ... --audit-log`; AGENTS.md required a validation report without a template; outdated architecture docs. | Docs rewritten from the code; validation template added. | docs commit |
| L1 | Low | Service | Backup file names used only a timestamp and could collide. | Sequence number in the file name. | `c6acd4b` |

## Policy decision: free PL1/PL2

An intermediate state of the branch refused a PL1 above the 55 W processor base power without
`--allow-sustained-above-base`, and the watchdog refused it entirely. The owner decided that PL1
and PL2 must be freely selectable. Since `543ee40` every pair within
`5 <= PL1 <= PL2 <= 157 W` is accepted by the CLI, the watchdog and the installer; the old flag
is a hidden no-op for compatibility. The register write rule (lock, mask, enable bits, range) is
unchanged in core, bridge and driver.

## Independent review of the branch

A separate review agent checked the complete branch diff. It found one medium issue: the write
command carried no expected value, so the bridge compared against its own fresh read and a
firmware change between the CLI's snapshot and the write (or before a rollback) could be
overwritten. Fixed: `msr-write`/`mmio-write` take the caller's planned value, the bridge and the
simulation refuse and audit a mismatch, and `apply` and rollback pass the planned or attempted
value. Tests: `refuses_when_the_register_changed_since_the_callers_read`,
`firmware_change_after_the_race_check_is_never_overwritten`.

## Hardening beyond the findings

- Bridge output bounded to 4096 bytes, error messages sanitised to 1024 characters, labels to
  128 characters.
- CSV export protected against formula injection.
- Unicode-only arguments, `fsync` of audit records before every write, sorted JSON keys.
- Driver: ApiValidator, code analysis with DriverRecommendedRules, all-gates compile in CI.
- PSScriptAnalyzer for every PowerShell script.

## Watt columns

Plan:

```text
Path  PL1 before [W]  PL2 before [W]  PL1 set [W]  PL2 set [W]  Raw before -> after
```

Audit table:

```text
Time (UTC)  Source  Event  Phase  Register  PL1 before [W]  PL2 before [W]  PL1 set [W]  PL2 set [W]  Outcome  Reason / detail
```

JSON fields: `pl1_before_w`, `pl2_before_w`, `pl1_set_w`, `pl2_set_w`, `pl1_readback_w`,
`pl2_readback_w`. CSV header:

```text
line,timestamp_utc,source,event,phase,reason,register,pl1_before_w,pl2_before_w,pl1_set_w,pl2_set_w,pl1_readback_w,pl2_readback_w,outcome,previous,requested,readback,simulated,detail
```

## Open items

- Microsoft attestation signing of the driver; test signing only on a lab machine.
- Hardware validation H1–H10 from [TEST_PLAN.md](TEST_PLAN.md), recorded with
  [VALIDATION_REPORT_TEMPLATE.md](VALIDATION_REPORT_TEMPLATE.md).
- MCHBAR layout validation before `RaplMmioLayoutValidated` may be enabled.
- `HalGetBusDataByOffset` is obsolete (C28159 suppressed with justification); a PnP filter on
  the host bridge is the long-term replacement.
- A rollback obeys the write policy, so a firmware value outside it is not restored; `plan`
  warns in that case.
- Single CPU model and single package scope.
- Firmware, EC or DTT may override the limits at any time.
- The installer has no uninstall switch; removal uses `sc.exe stop` and `sc.exe delete`.
