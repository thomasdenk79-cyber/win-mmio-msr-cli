# AGENTS.md: Windows hardware handoff

## Objective

Maintain and validate the Rust CLI, Windows bridge, KMDF driver and opt-in watchdog service for
the Lenovo ThinkPad P16 Gen 2 with the Intel Core i9-13950HX. Keep the design fail-closed and
evidence-oriented. Start with [docs/HANDOFF.md](docs/HANDOFF.md) and
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Mandatory security rules

1. Never expose arbitrary MSR numbers, PCI addresses or physical-memory addresses through an IOCTL,
   the bridge or the CLI.
2. Never use RwDrv, WinRing0 or another blocked universal driver. Never disable HVCI, Secure
   Boot, driver signature enforcement or the vulnerable-driver blocklist.
3. The driver whitelist is MSR `0x606` read, MSR `0x610` read and compare-and-write
   (`RaplEnableLiveWrites`) and `MCHBAR+0x59A0` read (`RaplEnableMmioRead`) and compare-and-write
   (additionally `RaplMmioLayoutValidated`). Every gate is off by default.
4. Discover the MCHBAR base from the PCI configuration of the host bridge. Never copy a physical
   address from sample code.
5. Validate every IOCTL buffer size, protocol version, register identifier, `Reserved` field,
   expected value and write mask. Refuse unsupported CPUs and multi-package systems.
6. Snapshot before writing. Preserve the lock, clamp, time-window and reserved bits. Write only
   when the register still holds the planned value, and read back after writing.
7. Stop on thermal alarms, WHEA errors, EC or firmware overrides, unexpected register races or a
   failed rollback.
8. PL1 and PL2 are free parameters within `5 <= PL1 <= PL2 <= 157 W` (register encoding and
   maximum turbo power); do not add further rejections. The default is PL1 = 55 W and
   PL2 = 157 W. Watch temperatures and stop on thermal alarms.
9. Every register write is recorded in the append-only JSONL audit log before (`write_attempt`)
   and after (`write_result`) the write, with UTC timestamp, register, previous, requested and
   readback values, PL1/PL2 before, set and readback watts and the outcome. No write happens when
   the record cannot be written.
10. The watchdog service is manual and opt-in, bounded by a correction budget and an error limit,
    and halts instead of fighting the firmware. It must never become an unreviewed auto-reapply
    loop.
11. The policy is defined once in `core/src/policy.rs`. The driver mirrors it in
    `RaplWriteAllowed` (`driver/src/driver.c`); change both together with their tests.

## Required validation

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked --target x86_64-pc-windows-msvc
.\scripts\smoke.ps1
.\.github\scripts\powershell-lint.ps1     # needs PSScriptAnalyzer 1.25.0
.\scripts\build.ps1 -Driver               # needs Visual Studio 2022 and the WDK 10.0.26100
```

CI runs all of these plus the driver code analysis and the watchdog test against the real Service
Control Manager ([docs/TEST_PLAN.md](docs/TEST_PLAN.md)).

Hardware evidence goes into a copy of
[docs/VALIDATION_REPORT_TEMPLATE.md](docs/VALIDATION_REPORT_TEMPLATE.md): CPU, BIOS and EC,
Windows build, Secure Boot and HVCI, the signed driver, raw register values compared with HWiNFO
and ThrottleStop, temperatures, WHEA and audit-log samples, reviewed by someone other than the
author.

Do not claim hardware readiness while driver signing, MMIO verification or the reviewed validation
report is missing.
