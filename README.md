# win-mmio-msr-cli

A fail-closed Windows toolchain for inspecting and, after independent validation, controlling Intel package RAPL limits through MSR and platform-specific MMIO. The target platform is a Lenovo ThinkPad P16 Gen 2 with an Intel Core i9-13950HX.

The repository has four layers:

- `win-mmio-msr-cli`: policy, unit conversion, snapshots, plans, backups, readback and rollback.
- `bridge/rapl-bridge.exe`: a small Windows process with a fixed register whitelist and durable audit logging.
- `driver/rapl-pkg.sys`: a KMDF/WDK driver with strict IOCTL validation. The checked-in build is read-only; live writes are gated.
- `service/rapl-watchdog.exe`: an explicit opt-in Windows service that detects changed limits and records every correction.

## Safety status

The checked-in driver can read MSR `0x606` and `0x610`. It refuses MMIO until the real PCI MCHBAR base, register layout and 64-bit access order have been validated on the target machine. `RAPL_ENABLE_LIVE_WRITES=0` is the default. No security feature, HVCI, Secure Boot or vulnerable-driver blocklist may be disabled.

157 W is the processor's maximum turbo power, not a generally safe sustained PL1. The service defaults to PL1=55 W and PL2=157 W and rejects a sustained PL1 above 55 W unless a separate policy change is made.

## CLI

```powershell
cargo test --all-targets
cargo build --release
.\target\release\win-mmio-msr-cli.exe status --mock --color always
.\target\release\win-mmio-msr-cli.exe plan --mock --preset base --json
.\target\release\win-mmio-msr-cli.exe apply --mock --preset base --backup "$env:TEMP\rapl-before.txt" --yes
```

For a validated Windows bridge:

```powershell
.\target\release\win-mmio-msr-cli.exe status --bridge C:\Tools\rapl-bridge.exe --audit-log C:\ProgramData\WinMmioMsr\rapl-audit.jsonl
.\target\release\win-mmio-msr-cli.exe apply --bridge C:\Tools\rapl-bridge.exe --preset base --audit-log C:\ProgramData\WinMmioMsr\rapl-audit.jsonl --backup C:\Temp\rapl-before.txt --yes
```

`--color auto|always|never` controls the readable ANSI terminal view. `--json` emits a stable `schema_version: 1` document for automation.

## Audit evidence

Set `--audit-log` for live bridge calls. The bridge appends one JSON object per event and flushes it to disk:

- `write_attempt`: current value and requested value before a real write;
- `write_result`: readback value and whether the register changed;
- `write_unchanged`: a write was requested but no register write was needed;
- service events: detection, backup, verified correction, errors and start/stop.

Keep the original JSONL file and the referenced backups unchanged. Copy them read-only when sending evidence to Lenovo.

## Windows service

Build the service and bridge for `x86_64-pc-windows-msvc`, install the signed driver, then run PowerShell as Administrator:

```powershell
.\scripts\install-service.ps1 -InstallRoot 'C:\Program Files\WinMmioMsr'
Start-Service RaplWatchdog
Get-Content "$env:ProgramData\WinMmioMsr\rapl-audit.jsonl" -Wait
```

The service is installed as Manual. It never starts or reapplies limits unless explicitly started. See [docs/SERVICE.md](docs/SERVICE.md).

## Architecture and handoff

- [Bridge and driver architecture](docs/BRIDGE_DRIVER_ARCHITECTURE.md)
- [Bridge protocol](docs/BRIDGE_PROTOCOL.md)
- [Service and audit evidence](docs/SERVICE.md)
- [Windows-agent handoff](docs/HANDOFF.md)
- [Validation test plan](docs/TEST_PLAN.md)
- [Driver build and signing](driver/README.md)

The real hardware bridge, MCHBAR verification and signed-driver validation remain explicit acceptance gates. No release is hardware-ready until those gates are documented.
