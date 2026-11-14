# Bridge protocol 2

`rapl-bridge.exe` is the only process that opens the driver device `\\.\RaplPkg`. The CLI and the
watchdog start it once per register operation. The bridge owns the device contract, the driver
gates, the write policy check and the audit record of every write. The callers own planning,
backups, readback comparison and rollback.

## Commands

| Command | Output on success | Device handle |
|---|---|---|
| `info` | `key=value` lines, see below | `GENERIC_READ` |
| `msr-read 0x606` | MSR `0x606` as `0x` and 16 hexadecimal digits | `GENERIC_READ` |
| `msr-read 0x610` | MSR `0x610` | `GENERIC_READ` |
| `mmio-read 0x59a0` | `MCHBAR+0x59A0` | `GENERIC_READ` |
| `msr-write 0x610 0xVALUE` | `OK` | `GENERIC_READ \| GENERIC_WRITE` |
| `mmio-write 0x59a0 0xVALUE` | `OK` | `GENERIC_READ \| GENERIC_WRITE` |

- Arguments must be valid Unicode. Numbers are strict `0x`-prefixed hexadecimal values with 1 to
  16 digits, without sign or spaces. Any other command or register is refused before the device
  is opened.
- Success: exit code 0 and the result on stdout. Failure: exit code 1 and one line on stderr
  (control characters removed, at most 1024 characters). Known Win32 errors of the driver are
  explained, see [driver/README.md](../driver/README.md#error-codes).
- The callers accept at most 4096 bytes per stream and stop the bridge after the timeout
  (default 10 s, CLI option `--timeout-ms`). A timed-out write has an unknown outcome; check the
  audit log and `status`.

## `info`

Output of a default (read-only) driver build:

```text
protocol=2
vendor=GenuineIntel
model=Intel(R) Core(TM) i9-13950HX
cpu_family=0x6
cpu_model=0xb7
cpu_stepping=0x1
packages=1
cpu_supported=true
mmio_readable=false
mmio_59a0_64bit_verified=false
live_writes_enabled=false
mchbar_base=unavailable
```

| Key | Source | Meaning |
|---|---|---|
| `protocol` | bridge | Always `2`; the CLI refuses every other version |
| `vendor` | driver (CPUID) | `GenuineIntel` or `unknown` |
| `model` | registry `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0`, value `ProcessorNameString` | Processor name, sanitized |
| `cpu_family`, `cpu_model`, `cpu_stepping` | driver (CPUID) | Hexadecimal |
| `packages` | driver | Number of physical packages |
| `cpu_supported` | driver flag `CPU_SUPPORTED` | The driver's CPU gate passed at load |
| `mmio_readable` | driver flag `MMIO_READABLE` | MMIO read gate on and MCHBAR found and enabled |
| `mmio_59a0_64bit_verified` | driver flag `MMIO_VERIFIED` | MMIO layout gate on |
| `live_writes_enabled` | driver flag `READ_ONLY` cleared | Live write gate on |
| `mchbar_base` | driver | `0x` and 16 digits, or `unavailable` |

The parser (`rapl-core::protocol::parse_info`) ignores unknown keys for forward compatibility. It
rejects missing, empty or duplicate keys, malformed values, a verified layout without MMIO read
access and MMIO read access without an MCHBAR base.

## Writes

For `msr-write` and `mmio-write` the bridge:

1. Requires `RAPL_AUDIT_LOG`.
2. Reads `info` and refuses when the CPU is unsupported, the driver is read-only or, for MMIO,
   the register is not both readable and validated.
3. Reads the units (MSR `0x606`) and the current register value.
4. When the requested value equals the current value, appends `write_unchanged`, sends no write
   IOCTL and prints `OK`.
5. Checks the transition with `check_register_write` (`rapl-core::policy`). A violation is
   appended as `write_refused` and the command fails.
6. Appends `write_attempt` and flushes it to disk. Without this record no write happens.
7. Sends a compare-and-write IOCTL with the value read in step 3 as the expected value. The
   driver re-reads the register, returns `STATUS_RETRY` when it changed, repeats the policy check,
   writes and returns its readback.
8. Appends `write_result` with the readback and the outcome `changed`, `readback_mismatch` or
   `driver_error`, and fails unless the outcome is `changed`.

## Audit environment

| Variable | Required | Meaning |
|---|---|---|
| `RAPL_AUDIT_LOG` | for writes | JSONL file; created with its parent directories when missing |
| `RAPL_AUDIT_REASON` | no | Label for the records; control characters are removed, at most 128 characters |
| `RAPL_AUDIT_PHASE` | no | `apply` or `rollback`; every other value is an error |

The CLI sets the variables from `--audit-log`, `--reason` and the current apply or rollback
phase; the watchdog uses the reason `watchdog_correction`. Variables that the caller does not set
are removed from the bridge environment, so inherited values cannot redirect or relabel records.

## Audit records

Every record is one JSON object per line with sorted keys. Common fields: `schema` (2),
`timestamp_utc` (RFC 3339, milliseconds), `source` (`rapl-bridge`, `rapl-watchdog`, or `mock` for
the simulation), `pid` and `event`.

| Field | Content |
|---|---|
| `command` | `msr-write` or `mmio-write` |
| `register` | `MSR_0x610` or `MMIO_MCHBAR_0x59A0` |
| `previous`, `requested`, `readback` | Raw register values; `readback` is `null` in `write_attempt` and `write_refused` and after a driver error |
| `pl1_before_w`, `pl2_before_w` | PL1/PL2 in watts before the change |
| `pl1_set_w`, `pl2_set_w` | PL1/PL2 in watts that were requested |
| `pl1_readback_w`, `pl2_readback_w` | PL1/PL2 in watts read back after the write |
| `outcome` | `pending`, `changed`, `readback_mismatch`, `driver_error`, `no_change` or `refused` |
| `phase` | `apply` or `rollback` |
| `reason` | Label from `RAPL_AUDIT_REASON` |
| `error` | Sanitized error text of `driver_error` and `write_refused` |
| `simulated` | `true` for records of the simulation backend |

Example from the simulation backend (the bridge writes the same fields with
`"source":"rapl-bridge"` and without `simulated`):

```json
{"command":"mmio-write","event":"write_result","outcome":"changed","phase":"apply","pid":73455,"pl1_before_w":55.0,"pl1_readback_w":50.0,"pl1_set_w":50.0,"pl2_before_w":157.0,"pl2_readback_w":120.0,"pl2_set_w":120.0,"previous":"0x004384e800dd81b8","readback":"0x004383c000dd8190","reason":"manual_test","register":"MMIO_MCHBAR_0x59A0","requested":"0x004383c000dd8190","schema":2,"simulated":true,"source":"mock","timestamp_utc":"2026-11-14T03:21:42.404Z"}
```

`win-mmio-msr-cli audit <log>` renders these records as a table or as CSV; records of schema 1
are shown with empty watt columns.

## Driver contract

The IOCTLs, structures and gates are defined in
[driver/include/rapl_ioctl.h](../driver/include/rapl_ioctl.h) and documented in
[driver/README.md](../driver/README.md). `bridge/src/abi.rs` mirrors the header; the test
`driver_header_matches_the_rust_contract` fails when they differ.
