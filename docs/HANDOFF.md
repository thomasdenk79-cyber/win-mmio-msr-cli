# Handoff for the Windows hardware work

## State

- The Rust workspace (CLI, bridge, watchdog, shared `rapl-core`) is complete for the simulation
  and is tested on Linux and Windows in CI, including a CLI smoke test and a watchdog installation
  against the real Service Control Manager.
- The KMDF driver builds in CI with warnings as errors, code analysis (Driver Recommended Rules),
  ApiValidator and InfVerif. The package is unsigned and has never run on hardware.
- All driver gates are off: the default build only reads MSR `0x606` and `0x610`.
- `plan`, `apply` and `audit` show PL1/PL2 before and set in watts for every register path; the
  audit log stores them for every register change.

The review findings and their status are listed in [ARCHITECTURE_REVIEW.md](ARCHITECTURE_REVIEW.md).

## Evaluation state (paused)

The owner approved evaluation with a test-signed driver on the target device; attestation signing
is not available. Done: the default read-only driver (all gates off, CI commit `9b8cefd`) was
test-signed with `scripts\eval-testsign.ps1` (local, non-exportable test certificate),
registered as kernel service `rapl-pkg` (demand start, not started) and test signing was
enabled. Pending: reboot, then `sc.exe start rapl-pkg` and a read-only `status`. Rollback:
`scripts\eval-rollback.ps1` stops and deletes the service, removes
`System32\drivers\rapl-pkg.sys` and the test certificate (My, Root, TrustedPublisher) and runs
`bcdedit /set testsigning off`; reboot afterwards. Next steps: [TODO.md](TODO.md).

## Build and test

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --workspace --release --locked --target x86_64-pc-windows-msvc
.\scripts\smoke.ps1
.\scripts\build.ps1 -Driver      # same checks plus the driver package (needs the WDK)
```

## Hardware work

Record every step in a copy of [VALIDATION_REPORT_TEMPLATE.md](VALIDATION_REPORT_TEMPLATE.md).

1. Record the system identity: CPUID, processor name, BIOS and EC versions, Windows build, Secure
   Boot, HVCI/VBS and the vulnerable-driver blocklist state.
2. Obtain a Microsoft-signed build of the default (read-only) driver package, for example through
   attestation signing. Never disable Secure Boot, HVCI or driver signature enforcement.
3. Install it (see [driver/README.md](../driver/README.md)) and compare `status` three times
   against HWiNFO and ThrottleStop for MSR `0x606` and `0x610`.
4. Build with `RaplEnableMmioRead=1`, sign, and collect the PCI MCHBAR evidence (host bridge
   `0:0.0` offset `0x48`, enable bit, base) and three read-only comparisons of `MCHBAR+0x59A0`.
5. Only after an independent review of the report: build with `RaplMmioLayoutValidated=1` and
   `RaplEnableLiveWrites=1`, then test `apply` with a small change, the rollback and the audit
   records.
6. Test the watchdog last, with a low correction budget.

## Stop criteria

Stop and document instead of continuing when values differ between tools or reads, firmware or
the EC changes the limits unexpectedly, a thermal alarm or WHEA error occurs, a rollback fails, or
any step would require disabling a Windows security feature.

## Evidence

Use protected paths such as `C:\ProgramData\WinMmioMsr\rapl-audit.jsonl` and
`C:\ProgramData\WinMmioMsr\backups\`. Keep the original audit log and all backups unchanged and
attach read-only copies and `win-mmio-msr-cli audit <log> --csv` exports to the report.
