# AGENTS.md — Windows hardware handoff

## Objective

Complete the existing Rust CLI, Windows bridge, KMDF driver and opt-in watchdog service for the Lenovo ThinkPad P16 Gen 2 / Intel Core i9-13950HX. Keep the design fail-closed and evidence-oriented.

## Mandatory security rules

1. Never expose arbitrary MSR numbers, PCI addresses or physical-memory addresses through an IOCTL or CLI.
2. Never use RwDrv, WinRing0 or another blocked universal driver. Never disable HVCI, Secure Boot, signature enforcement or the vulnerable-driver blocklist.
3. The driver whitelist is MSR `0x606` read, MSR `0x610` read/write behind the build gate, and the validated MCHBAR package-limit register only.
4. Discover the real MCHBAR BAR from PCI configuration. Never copy a physical address from sample code.
5. Validate every IOCTL size, version, enumeration, Reserved field, package/CPU context, expected value and write mask.
6. Snapshot before writing. Preserve lock, clamp, time-window and Reserved bits. Read back after writing.
7. Stop on thermal alarms, WHEA, EC/firmware overrides, unexpected register races or failed rollback.
8. Sustained PL1=157 W is not an automatic test. The default policy is PL1=55 W, PL2=157 W.
9. Every actual change must be recorded in the append-only JSONL audit log with UTC timestamp, register, previous value, requested value, readback and outcome.
10. The watchdog service is manual and opt-in. It must never become an unreviewed auto-reapply loop.

## Required validation

- `cargo test --all-targets`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --manifest-path bridge/Cargo.toml`
- `cargo test --manifest-path service/Cargo.toml`
- Windows MSVC release builds for CLI, bridge and service
- WDK/KMDF build, INF/catalog generation and signature verification
- Read-only comparison of MSR 0x606/0x610 and MMIO against HWiNFO and ThrottleStop
- `docs/VALIDATION_REPORT.md` with CPU, BIOS, Windows build, Secure Boot/HVCI, raw values, temperatures, WHEA and audit-log samples

Do not claim hardware readiness while MMIO verification or driver signing is missing.
