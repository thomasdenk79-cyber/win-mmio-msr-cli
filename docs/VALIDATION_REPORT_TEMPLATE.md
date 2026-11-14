# Validation report

Copy this file to `docs/VALIDATION_REPORT.md` (or attach it to the change) and fill in every
field. Leave nothing implicit: write "not tested" instead of deleting a row. Store raw evidence
(logs, screenshots, hashes) read-only next to the report. Never include secrets.

## 1. System identity

| Item | Value |
|---|---|
| Device model and serial (redacted if required) | |
| BIOS/UEFI version and date | |
| EC firmware version | |
| Windows edition and build | |
| Secure Boot state | |
| HVCI / VBS state | |
| Vulnerable driver blocklist state | |
| CPU brand string | |
| CPUID family / model / stepping | |
| Other power tools running (DTT, vendor tools, ThrottleStop) | |

## 2. Build under test

| Item | Value |
|---|---|
| Repository commit SHA | |
| Driver gates (`RaplEnableMmioRead`, `RaplMmioLayoutValidated`, `RaplEnableLiveWrites`) | |
| Driver signature (attestation or test signing) | |
| SHA-256 of `rapl-pkg.sys`, `rapl-pkg.inf`, catalog | |
| SHA-256 of `rapl-bridge.exe`, `win-mmio-msr-cli.exe`, `rapl-watchdog.exe` | |
| CI run URL for the commit | |

## 3. PCI and MCHBAR evidence

| Item | Value |
|---|---|
| Host bridge vendor/device ID (bus 0, device 0, function 0) | |
| MCHBAR register raw value and decoded base | |
| Source used to confirm the layout (datasheet section, tool) | |
| Offset 0x59A0 confirmed as package power limit | yes / no |

## 4. Read-only comparison

Repeat three times, at idle and under load. Values must match within one power unit.

| Run | Load | CLI MSR 0x606 units | CLI MSR 0x610 PL1/PL2 [W] | CLI MMIO 0x59A0 PL1/PL2 [W] | Reference tool PL1/PL2 [W] | Match |
|---|---|---|---|---|---|---|
| 1 | | | | | | |
| 2 | | | | | | |
| 3 | | | | | | |

Reference tool and version: 

## 5. Write tests

Every write needs an audit log entry pair (`write_attempt` + `write_result`) and a readback.

| Test | Command | PL1/PL2 before [W] | PL1/PL2 set [W] | Readback [W] | Audit lines | Result |
|---|---|---|---|---|---|---|
| Lower limits | | | | | | |
| Restore defaults | | | | | | |
| No-op apply (no write expected) | | | | | | |
| Policy refusal (PL1 > PL2) | | | | | | |

## 6. Rollback test

| Item | Value |
|---|---|
| Induced mismatch method | |
| `phase=rollback` records present | yes / no |
| Registers after rollback | |

## 7. Watchdog test

| Item | Value |
|---|---|
| Install command and parameters | |
| Correction detected and verified (audit lines) | |
| Halt after correction budget or error streak (exit code) | |
| Removal (`sc.exe stop`, `sc.exe delete`) | |

## 8. Thermal and stability

| Item | Value |
|---|---|
| Load tool and duration | |
| Peak package temperature | |
| Throttling observed | |
| WHEA events (Event Viewer) | |
| Bug checks / unexpected restarts | |

## 9. Decision

- [ ] Approved for this machine and build
- [ ] Approved with restrictions: 
- [ ] Rejected: 

Open issues:

## 10. Sign-off

The reviewer must not be the author of the report.

| Role | Name | Date | Signature / commit |
|---|---|---|---|
| Author | | | |
| Reviewer | | | |
