# Architecture

## Components

| Crate (folder) | Artifact | Responsibility |
|---|---|---|
| `rapl-core` (`core/`) | library | RAPL register model (`rapl`), the single PL1/PL2 policy (`policy`), the JSONL audit format and the audited write (`audit`), the bridge `info` protocol (`protocol`) |
| `win-mmio-msr-cli` (`/`) | `win-mmio-msr-cli.exe` | `status`, `plan`, `apply` with backup, readback and rollback, `audit` viewer, JSON and CSV output, simulation backend |
| `rapl-bridge` (`bridge/`) | `rapl-bridge.exe` | The only process that opens `\\.\RaplPkg`: command whitelist, driver gates, policy check, audit record before every write |
| - (`driver/`) | `rapl-pkg.sys` | KMDF control device: fixed register whitelist, compare-and-write, independent policy copy, build-time gates |
| `rapl-watchdog` (`service/`) | `rapl-watchdog.exe` | Opt-in Windows service that re-applies the target limits within a correction budget |

```text
win-mmio-msr-cli.exe --+
rapl-watchdog.exe -----+-- HardwareBackend
                           |-- MockBackend     --mock: in-memory simulation, same policy and audit records
                           `-- ExternalBridge  one bridge process per call, timeout, bounded output
                                 |  arguments + RAPL_AUDIT_LOG / RAPL_AUDIT_REASON / RAPL_AUDIT_PHASE
                               rapl-bridge.exe  gates, policy, audit, least-privilege device handle
                                 |  DeviceIoControl, protocol 2
                               \\.\RaplPkg -> rapl-pkg.sys
                                 |-- MSR 0x606       read
                                 |-- MSR 0x610       read, write (gate)
                                 `-- MCHBAR+0x59A0   read (gate), write (gates)
```

No component accepts a caller-supplied MSR number or physical address. The bridge maps
`msr-read 0x606`, `msr-read 0x610`, `mmio-read 0x59a0`, `msr-write 0x610` and `mmio-write 0x59a0`
to register enumerations; the driver derives MCHBAR from PCI configuration.

## One policy, three enforcement points

`rapl-core::policy` is the single source of the limits:

- `validate_limits` checks a request: `5 <= PL1 <= PL2 <= 157 W`. Any pair in this range is
  accepted; a sustained PL1 above the 55 W processor base power needs no extra flag.
- `check_register_write` checks a raw register transition: the register is not locked, only the
  PL1/PL2 power and enable fields (mask `0x0000FFFF0000FFFF`) change, both limits stay enabled
  and the decoded limits satisfy `5 <= PL1 <= PL2 <= 157 W`.

| Enforcement point | Check | On violation |
|---|---|---|
| CLI and watchdog (`build_plan`) | `validate_limits`, then `check_register_write` for every register that changes | error, or a plan blocker; nothing is written |
| Bridge (`write`) | driver gates, then `check_register_write` against the value it just read | `write_refused` audit record, exit code 1 |
| Driver (`RaplWriteAllowed`) | the same register rule in integer arithmetic, against the value read inside the driver | `STATUS_INVALID_PARAMETER` |

The driver constants live in `driver/include/rapl_ioctl.h`. The bridge test
`driver_header_matches_the_rust_contract` parses the header and fails when it drifts from
`bridge/src/abi.rs` or from the policy constants.

## Register model

- MSR `0x606` holds the RAPL units; the power unit is `1 / 2^(bits 3:0)` W.
- MSR `0x610` and `MCHBAR+0x59A0` share one layout: PL1 power (bits 14:0), PL1 enable (15),
  PL1 clamp (16), PL1 time window (23:17), PL2 power (46:32), PL2 enable (47), PL2 clamp (48),
  PL2 time window (55:49) and lock (63).
- A change rewrites only the power and enable fields. Clamp, time windows, lock and reserved bits
  keep their current values; a locked register blocks the plan.
- The effective limit can still be lower: BIOS, embedded controller or Intel DTT may enforce their
  own limits or overwrite the registers.

## Simulation backend

`--mock` selects `MockBackend`: an in-memory i9-13950HX with a 1/8 W power unit and 55/157 W
limits. The identity is flagged `simulated`, so the CLI prints `SIMULATION / NO HARDWARE` and the
audit log marks records with `simulated: true` (`[SIM]` in the table). The backend applies the same
register policy and writes the same audit records as the bridge. Its state lives for one process.
The unit tests and the CI smoke test (`scripts/smoke.ps1`) use it.

## Plan

`plan` is read-only:

1. Validate the requested limits.
2. Read the identity and refuse unsupported systems (`validate_identity`: GenuineIntel, family 6,
   model `0xB7`, processor name containing `i9-13950HX`, the driver's CPU gate and exactly one
   package).
3. Read the units, MSR `0x610` and, when the driver can read it, `MCHBAR+0x59A0`.
4. Compute the target value of each register path. The plan shows PL1/PL2 before and set in
   watts plus the raw transition for each path.
5. Collect blockers (MMIO not readable, MMIO layout not validated, driver read-only, policy
   violation) and warnings (a rollback to the current value would be refused by the policy).

`status` works without MMIO access; only `apply` needs both paths.

## Apply

1. Refuse when the plan has blockers. When both registers already hold the target, write nothing,
   create no backup and report `changed: false`.
2. Re-read identity, units and both registers; abort without writing on any difference to the
   plan.
3. Create the backup with `create_new` and flush it (`WIN_MMIO_MSR_BACKUP_V2`: identity, units,
   raw values and watts of both paths, target values). An existing file is never overwritten.
4. Phase `apply`: write MMIO, then MSR, then read both back and compare them with the targets.
5. On any error or mismatch, phase `rollback`: restore MSR, then MMIO. A register is restored only
   while it still holds the attempted value, and the restore is read back. Any other value is
   reported instead of overwritten, because firmware may have changed it.

The bridge refuses a rollback that violates the policy, for example when firmware had programmed
a PL1 below 5 W. The plan warns about this case in advance.

## Audit trail

- JSONL, append-only. Each record is written with one `write_all` call and flushed with
  `sync_all` before the next step. Records carry `schema: 2`, `timestamp_utc` (UTC, milliseconds),
  `source`, `pid` and `event`.
- The bridge persists `write_attempt` before every hardware write, so an unwritable audit log
  prevents the write. The outcome follows as `write_result` (`changed`, `readback_mismatch`,
  `driver_error`), `write_unchanged` or `write_refused`.
- Register records carry the raw `previous`, `requested` and `readback` values and the readable
  columns `pl1_before_w`, `pl2_before_w`, `pl1_set_w`, `pl2_set_w`, `pl1_readback_w` and
  `pl2_readback_w`, plus `phase` (`apply` or `rollback`) and `reason`.
- The watchdog adds `service_*`, `correction_detected` and `correction_verified` records.
- `win-mmio-msr-cli audit <log>` shows the records as a table, as CSV (`--csv`, cells quoted and
  protected against spreadsheet formulas) or as JSON, optionally only the newest N (`--last N`).
  Malformed lines are counted and reported, never dropped silently.

## Trust boundaries

| Boundary | Protection |
|---|---|
| User -> CLI | `--mock` needs no privileges. Live access needs an elevated process because the device admits only SYSTEM and Administrators. `apply` needs `--yes`, a new backup file and, with `--bridge`, an audit log. |
| CLI or watchdog -> bridge | The bridge path must resolve to an existing `.exe`. Each call has a timeout (default 10 s, `--timeout-ms`) and at most 4096 bytes of output. Unset audit variables are removed from the child environment, so inherited values cannot redirect or relabel records. The bridge re-validates every argument. |
| Bridge -> driver | Protocol 2 with exact buffer sizes. Reads open the device with `GENERIC_READ`, writes with `GENERIC_READ | GENERIC_WRITE`; the IOCTL codes require `FILE_READ_DATA` or `FILE_WRITE_DATA`. Writes carry the caller's planned value: the bridge refuses when the register no longer holds it, and the driver's compare-and-write returns `STATUS_RETRY` when it changed in between. |
| Driver -> hardware | Fixed register whitelist, CPU gate at load, build-time gates, MCHBAR from PCI `0:0.0` offset `0x48`, mapping only for the duration of one access, `rdmsr`/`wrmsr` faults caught. |
| Service | Runs as LocalSystem. The installer refuses binaries and directories that a non-administrator could modify, replace or redirect. |

## Scope and limits

- One CPU model (i9-13950HX, Raptor Lake HX) on one package. Other systems are refused.
- The register layout of `MCHBAR+0x59A0` is not validated on the target machine; the MMIO read and
  write gates stay off until a validation report covers them.
- The rollback obeys the write policy and therefore cannot restore every firmware value.
- Software outside this toolchain can change the registers at any time. Re-reads, the driver's
  compare-and-write and the watchdog's correction budget detect this; they do not prevent it.
