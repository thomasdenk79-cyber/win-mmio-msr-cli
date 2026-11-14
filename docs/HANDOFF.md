# Handoff for the local Windows agent

The repository contains the policy CLI, fixed-whitelist bridge, fail-closed KMDF driver core and manual watchdog service. The driver reads MSR 0x606/0x610. MMIO and live writes remain gated until the target platform is independently validated.

## Local build

```powershell
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --manifest-path bridge/Cargo.toml
cargo test --manifest-path service/Cargo.toml
cargo build --release
cargo build --manifest-path bridge/Cargo.toml --release --target x86_64-pc-windows-msvc
cargo build --manifest-path service/Cargo.toml --release --target x86_64-pc-windows-msvc
```

## Hardware work

1. Build the KMDF driver with WDK/MSVC and keep `RAPL_ENABLE_LIVE_WRITES=0`.
2. Verify CPUID, exact processor name, chipset, BIOS/EC, Windows build, Secure Boot and HVCI/VBS.
3. Discover and validate the real PCI MCHBAR BAR and the package-limit register at offset 0x59A0/0x59A4.
4. Capture three independent read-only comparisons against HWiNFO/ThrottleStop.
5. Install only a correctly signed driver; do not bypass Windows security.
6. Review the audit JSONL and backup behavior before enabling any write gate.
7. Add `docs/VALIDATION_REPORT.md` with raw before/after values, temperatures, WHEA results and signature hashes.

## Evidence collection

Use a protected path such as `C:\ProgramData\WinMmioMsr\rapl-audit.jsonl`. Preserve the original JSONL file and all backup files. Every `write_attempt`, `write_result`, `write_unchanged`, `correction_detected` and `correction_verified` entry must have a UTC timestamp and register values.
