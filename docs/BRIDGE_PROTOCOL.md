# Windows bridge protocol v1

The CLI owns policy, conversion, snapshots, backups and rollback. `rapl-bridge.exe` owns only the narrow privileged device contract. It opens `\\.\RaplPkg` and never accepts a raw MSR number or physical address.

## Commands

```text
rapl-bridge.exe info
rapl-bridge.exe msr-read 0x606
rapl-bridge.exe msr-read 0x610
rapl-bridge.exe mmio-read 0x59a0
rapl-bridge.exe msr-write 0x610 0x0123456789abcdef
rapl-bridge.exe mmio-write 0x59a0 0x0123456789abcdef
```

`info` returns UTF-8 `key=value` lines:

```text
protocol=1
vendor=GenuineIntel
model=Intel(R) Core(TM) i9-13950HX
mmio_59a0_64bit_verified=false
```

Reads return exactly one `0x`-prefixed 64-bit value. Successful writes return exactly `OK`. Diagnostics go to stderr.

## Audit environment

The CLI sets `RAPL_AUDIT_LOG` and `RAPL_AUDIT_REASON`. The bridge appends JSONL records with UTC timestamp, command, register, previous value, requested value, readback, outcome, process ID and reason. It flushes the file after each event. A changed write produces `write_attempt` and `write_result`; an equal value produces `write_unchanged` and no hardware write.

## Required gates

- `info` may report MMIO verified only after CPUID, chipset, PCI BAR, enable bit, register layout and independent comparison measurements pass.
- The driver uses a fixed enumeration whitelist and validates structure size, version, Reserved fields, lock bits and ExpectedValue.
- Access is administrator/SYSTEM only. HVCI, Secure Boot, signature enforcement and the vulnerable-driver blocklist remain enabled.
- Unknown CPU, locked register, race, invalid mapping or failed readback is an error.
- No universal or third-party kernel driver is bundled.
