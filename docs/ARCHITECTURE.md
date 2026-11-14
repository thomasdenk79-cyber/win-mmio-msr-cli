# Architecture

```text
win-mmio-msr-cli.exe
  policy, RAPL units, plan, backup, readback, rollback
        |
  HardwareBackend
    |-- MockBackend (tests only)
    `-- ExternalBridge
          |
      rapl-bridge.exe
          |
      \\.\RaplPkg
          |
      signed KMDF rapl-pkg.sys
          |-- MSR 0x606 read
          |-- MSR 0x610 read / gated write
          `-- validated MCHBAR+0x59A0 only
```

The CLI never receives a physical address. The bridge accepts only register enumerations and the driver repeats all validation. The service uses the same backend and writes the same audit file.

## RAPL register model

- MSR 0x606 supplies the power and time units.
- MSR 0x610 contains PL1/PL2 values, enable bits, clamp bits, time windows and the lock bit.
- MMIO uses the same layout only after the target platform's MCHBAR register is independently verified.
- Read-modify-write changes only the approved power and enable fields.

## Apply sequence

1. Read identity, units and both register paths.
2. Validate policy limits.
3. Build and display a plan.
4. Read again immediately before applying.
5. Create an exclusive backup.
6. Write, read back and compare exact intended fields.
7. On failure, perform guarded rollback and report every result.
8. The bridge writes durable JSONL audit events for every real change.
