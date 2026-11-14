# Windows service and audit evidence

`rapl-watchdog.exe` is an explicit opt-in Windows service. It polls the validated bridge, compares the current MSR/MMIO power fields with the configured target, and applies a correction only when a field differs.

Every real bridge write produces append-only JSONL events:

- `write_attempt`: previous value and requested value
- `write_result`: readback value and `changed` outcome
- `write_unchanged`: no hardware write was needed
- `correction_detected` / `correction_verified`: service-level context and backup path
- `service_started`, `service_error`, `service_stopped`

The audit file is flushed with `sync_all()` after every event. Keep it on a protected NTFS path and copy it as evidence together with the per-correction backups. Do not edit the original file; make a read-only copy for Lenovo.

## Install

Build `rapl-watchdog.exe` and `rapl-bridge.exe` for `x86_64-pc-windows-msvc`, install the signed driver, then run PowerShell as Administrator:

```powershell
.\scripts\install-service.ps1 -InstallRoot C:\Program Files\WinMmioMsr
Start-Service RaplWatchdog
Get-Content "$env:ProgramData\WinMmioMsr\rapl-audit.jsonl -Wait
```

The service is installed as **Manual**. It does not auto-start until explicitly started. Stop it with `Stop-Service RaplWatchdog`; remove it with `sc.exe delete RaplWatchdog`.

The default target is PL1=55 W and PL2=157 W. A 157 W sustained PL1 is intentionally rejected by the service policy. The service must not be enabled before read-only comparison against HWiNFO/ThrottleStop and a signed driver validation report.
