# win-mmio-msr-cli

A fail-closed Windows toolchain that reads and, after independent hardware validation, sets the Intel package power limits PL1 and PL2 of a Lenovo ThinkPad P16 Gen 2 with an Intel Core i9-13950HX. The limits exist twice, in MSR `0x610` and in the MCHBAR register `0x59A0`. The tools keep both copies synchronized, back them up, verify every write by readback and audit every register change.

> **Status: not hardware-ready.** The driver is unsigned, the MCHBAR layout is not validated on the target machine and live writes are disabled at build time. Use `--mock` until the gates in [driver/README.md](driver/README.md) are closed and a [validation report](docs/VALIDATION_REPORT_TEMPLATE.md) is complete.

## Components

| Folder (crate) | Binary | Role |
|---|---|---|
| `core/` (`rapl-core`) | library | RAPL register model, write policy, audit record format, bridge text protocol |
| `/` (`win-mmio-msr-cli`) | `win-mmio-msr-cli.exe` | status, plan, apply with backup, readback and rollback, audit viewer |
| `bridge/` (`rapl-bridge`) | `rapl-bridge.exe` | the only process that opens `\\.\RaplPkg`: fixed register whitelist, policy check, audit record before every write |
| `driver/` | `rapl-pkg.sys` | KMDF control device: fixed register whitelist, compare-and-write, build-time gates |
| `service/` (`rapl-watchdog`) | `rapl-watchdog.exe` | opt-in service that re-applies PL1/PL2 within a correction budget |

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design and [docs/ARCHITECTURE_REVIEW.md](docs/ARCHITECTURE_REVIEW.md) for the review findings and their status.

## Build and test

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked --target x86_64-pc-windows-msvc
.\scripts\smoke.ps1
```

`scripts\build.ps1` runs the same steps. The driver needs Visual Studio 2022 with the Windows Driver Kit; see [driver/README.md](driver/README.md). CI builds and tests all parts, including the driver package and a service installation against the real Service Control Manager.

## Usage

The simulation backend (`--mock`) needs no driver and no administrator rights:

```powershell
$cli = '.\target\x86_64-pc-windows-msvc\release\win-mmio-msr-cli.exe'
& $cli status --mock
& $cli plan --mock --pl1 50 --pl2 120
& $cli apply --mock --pl1 50 --pl2 120 --yes --backup "$env:TEMP\rapl-before.txt" --audit-log "$env:TEMP\rapl-audit.jsonl"
& $cli audit "$env:TEMP\rapl-audit.jsonl"
```

`plan` and `apply` show the watts before and after the change for each register path:

```text
CHANGE PLAN
Target: PL1 50 W / PL2 120 W
Path  PL1 before [W]  PL2 before [W]  PL1 set [W]  PL2 set [W]  Raw before -> after
MMIO              55             157           50          120  0x004384e800dd81b8 -> 0x004383c000dd8190
MSR               55             157           50          120  0x004384e800dd81b8 -> 0x004383c000dd8190
```

`audit <log>` shows the same four watt columns for every logged register change, from the CLI, the bridge or the watchdog:

```text
Time (UTC)                Source      Event          Phase  Register            PL1 before [W]  PL2 before [W]  PL1 set [W]  PL2 set [W]  Outcome  Reason / detail
2026-11-14T02:58:56.460Z  mock [SIM]  write_attempt  apply  MMIO_MCHBAR_0x59A0              55             157           50          120  pending  cli_apply
2026-11-14T02:58:56.481Z  mock [SIM]  write_result   apply  MMIO_MCHBAR_0x59A0              55             157           50          120  changed  cli_apply
2026-11-14T02:58:56.483Z  mock [SIM]  write_attempt  apply  MSR_0x610                       55             157           50          120  pending  cli_apply
2026-11-14T02:58:56.483Z  mock [SIM]  write_result   apply  MSR_0x610                       55             157           50          120  changed  cli_apply
```

`--last N` keeps the newest N records. `--csv` also emits the readback watts and raw values (`pl1_before_w`, `pl2_before_w`, `pl1_set_w`, `pl2_set_w`, `pl1_readback_w`, `pl2_readback_w`, `previous`, `requested`, `readback`). Malformed lines are reported, never silently dropped.

### Rules

- Limits come from `--pl1`/`--pl2`, `--preset base` (55/157 W) or `--preset turbo` (157/157 W).
- `5 <= PL1 <= PL2 <= 157 W`; any pair in this range is accepted, including a sustained PL1 above the 55 W processor base power (no extra flag). The bounds come from the register encoding and the CPU's 157 W maximum turbo power; firmware or the EC may still enforce lower limits, so watch temperatures with a high PL1.
- `apply` requires `--yes` and a new `--backup` file; an existing file is never overwritten. With `--bridge`, `--audit-log` is mandatory; `--reason` labels the audit records.
- `apply` writes nothing when the registers already hold the target. Otherwise it re-reads both registers, aborts if anything changed since the plan, writes the backup, writes MMIO and then MSR, and reads both back. On a mismatch it rolls back MSR, then MMIO.
- Only the PL1/PL2 power and enable fields change; lock, clamp, time-window and reserved bits are preserved. A locked register blocks the plan.
- A rollback obeys the same write policy. If firmware had programmed a value outside it, the plan warns that a rollback would be refused and the new limits would stay active.
- `--json` prints one document with `schema_version: 2`. `--color auto|always|never` controls ANSI colors. `completions <shell>` prints a shell completion script.

With a validated driver and bridge, from an elevated PowerShell:

```powershell
$bridge = 'C:\Program Files\WinMmioMsr\rapl-bridge.exe'
& $cli status --bridge $bridge
& $cli plan --bridge $bridge --preset base
& $cli apply --bridge $bridge --preset base --yes --backup C:\ProgramData\WinMmioMsr\backups\manual-001.txt --audit-log C:\ProgramData\WinMmioMsr\rapl-audit.jsonl --reason manual_base
```

## Audit evidence

Before every register write the bridge appends a `write_attempt` record to the JSONL audit log and flushes it to disk. The outcome follows as `write_result` (`changed`, `readback_mismatch` or `driver_error`), `write_unchanged` (no write needed) or `write_refused` (policy violation). The watchdog adds `correction_detected`, `correction_verified` and `service_*` records. Each register record carries PL1/PL2 before, set and readback watts and the raw values; `phase` separates `apply` from `rollback`.

Keep the original log and the backups unchanged and hand over read-only copies.

## Watchdog service

`rapl-watchdog` is opt-in: manual start, LocalSystem, no automatic restart. It halts after its correction budget or a streak of errors instead of fighting the firmware. PL1 and PL2 are free parameters within `5 <= PL1 <= PL2 <= 157 W` (default 55/157 W). Install and remove it with [scripts/install-service.ps1](scripts/install-service.ps1); see [docs/SERVICE.md](docs/SERVICE.md).

## Safety rules

- Never disable Secure Boot, HVCI, driver signature enforcement or the vulnerable-driver blocklist, and never use universal drivers such as WinRing0 or RwDrv.
- The driver accepts no caller-supplied MSR number or physical address. It derives MCHBAR from PCI configuration.
- Live writes stay disabled until the validation report is complete and reviewed.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) and [architecture review](docs/ARCHITECTURE_REVIEW.md)
- [Bridge and driver](docs/BRIDGE_DRIVER_ARCHITECTURE.md), [bridge protocol 2](docs/BRIDGE_PROTOCOL.md)
- [Driver build, signing and gates](driver/README.md)
- [Watchdog service](docs/SERVICE.md)
- [Validation test plan](docs/TEST_PLAN.md) and [validation report template](docs/VALIDATION_REPORT_TEMPLATE.md)
- [Handoff for the Windows hardware work](docs/HANDOFF.md), [evaluation route and open work](docs/EVALUATION.md), [TODO](docs/TODO.md)
