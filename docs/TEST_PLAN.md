# Validation test plan

| Phase | Test | Expected result |
|---|---|---|
| 0 | Linux CLI and bridge/service tests | All tests, format and Clippy checks pass |
| 1 | `status --mock`, `plan --mock`, `apply --mock` | No hardware access; backup and rollback work |
| 2 | Bridge on non-Windows or without device | Clear failure; no register access |
| 3 | Unknown register/version/size/Reserved IOCTL | Rejected before hardware operation |
| 4 | Signed driver, read-only mode | MSR reads match independent tools; MMIO remains unverified |
| 5 | Firmware race before write | Operation aborts without writing |
| 6 | Corrected register value | JSONL attempt/result events, exact readback and backup |
| 7 | Same value requested | `write_unchanged`; no hardware write |
| 8 | Driver failure or timeout | Error is logged; rollback status is explicit |
| 9 | Watchdog service | Manual start only; correction creates backup and audit events |
| 10 | Suspend/resume, BIOS override, WHEA/thermal event | Stop or document; no blind reapply loop |

## Stop criteria

Unverified MCHBAR base, unsigned or blocked driver, required security disablement, inconsistent reads, unexpected firmware changes, thermal/EC alarm, WHEA or failed rollback.
