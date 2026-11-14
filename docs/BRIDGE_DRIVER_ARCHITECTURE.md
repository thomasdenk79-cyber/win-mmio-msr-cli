# Bridge and driver architecture

The privileged path is deliberately split:

1. `rapl-bridge.exe` is a small Windows process that validates the text protocol, opens only `\\.\RaplPkg`, serializes requests and writes audit events.
2. `rapl-pkg.sys` is a KMDF driver with a fixed IOCTL whitelist and no arbitrary-address API.

```text
CLI -> rapl-bridge.exe -> \\.\RaplPkg -> rapl-pkg.sys -> MSR / validated MCHBAR
```

The driver state is `UNVERIFIED -> READ_ONLY_VERIFIED -> WRITE_ENABLED`. The checked-in build stops at read-only mode. `RAPL_ENABLE_LIVE_WRITES=0` makes all write IOCTLs return `STATUS_NOT_SUPPORTED`; MMIO remains blocked until PCI MCHBAR discovery and 32-bit access ordering are documented.

## Security properties

- Register IDs are enumerations, never caller-supplied addresses.
- IOCTL sizes, version, Reserved fields, register IDs, ExpectedValue, lock and write masks are validated twice.
- MCHBAR mappings are temporary and must be derived from PCI configuration.
- The device ACL allows SYSTEM and administrators only.
- A signed WDK build is required. No Windows security feature may be disabled.

## Acceptance gate for live writes

Provide `docs/VALIDATION_REPORT.md` with exact CPU/chipset/BIOS data, PCI BAR evidence, three independent read-only comparisons, signature hashes, thermal/WHEA observations, rollback tests and audit samples. Only then may a separately reviewed build enable live writes.
