# Bridge and driver security design

The privileged path is split into a small user-mode bridge and a KMDF driver:

```text
CLI or watchdog -> rapl-bridge.exe -> \\.\RaplPkg -> rapl-pkg.sys -> MSR 0x606/0x610, MCHBAR+0x59A0
```

The bridge keeps everything that needs no kernel privileges out of the driver: argument parsing,
the processor name, the audit log and the error explanations. The driver keeps the minimum that
must be enforced in the kernel, independently of any user-mode code. The command set is described
in [BRIDGE_PROTOCOL.md](BRIDGE_PROTOCOL.md), the driver build and IOCTL contract in
[driver/README.md](../driver/README.md).

## Threats and controls

| Threat | Control |
|---|---|
| The device becomes a generic MSR or physical-memory primitive | Fixed register enumeration; no MSR number or address crosses the IOCTL boundary; MCHBAR comes from PCI `0:0.0` offset `0x48` and is mapped for a single 64-bit access |
| A non-administrator opens the device | SDDL `D:P(A;;GA;;;SY)(A;;GA;;;BA)` with `FILE_DEVICE_SECURE_OPEN`; write IOCTLs require `FILE_WRITE_DATA`, so a read-only handle cannot write |
| Malformed or stale requests | `METHOD_BUFFERED` with exact input and output sizes, protocol version and reserved fields checked, register valid for the IOCTL; mismatches return `STATUS_REVISION_MISMATCH` or `STATUS_INVALID_PARAMETER` |
| A compromised or buggy bridge writes unsafe values | The driver repeats the register policy (lock, protected bits, enable bits, `5 <= PL1 <= PL2 <= 157 W`) using the units it reads itself |
| Firmware changes the register between read and write | The CLI re-reads both registers before writing and passes the planned value with every write; the bridge refuses when the register no longer holds it, and the driver's compare-and-write (`STATUS_RETRY`) covers the last step. The readback is verified |
| Writes without evidence | The bridge persists `write_attempt` before the IOCTL; an unwritable audit log blocks the write |
| Wrong CPU or platform | CPU gate in the driver at load (GenuineIntel, family 6, model `0xB7`, one package) and identity checks in the CLI |
| Unvalidated MMIO layout | Build-time gates: MMIO read, MMIO layout validated and live writes are separate MSBuild properties, all off by default |
| Faulting `rdmsr`/`wrmsr` (for example under a hypervisor) | Structured exception handling returns `STATUS_IO_DEVICE_ERROR` instead of a bug check |
| Concurrent requests | Sequential, non-power-managed I/O queue at `PASSIVE_LEVEL` |

## Build-time gates

| Build | MSR `0x610` write | MMIO read | MMIO write | Use |
|---|---|---|---|---|
| Default (all gates off) | no | no | no | Read-only comparison of MSR values |
| `RaplEnableMmioRead=1` | no | yes | no | Read-only comparison of the MMIO copy and MCHBAR evidence |
| plus `RaplMmioLayoutValidated=1` and `RaplEnableLiveWrites=1` | yes | yes | yes | Only after a reviewed validation report |

`RaplEnableLiveWrites=1` alone enables MSR writes but the CLI still refuses `apply`, because it
keeps MSR and MMIO synchronized and requires both paths.

## Acceptance gate for live writes

A build with live writes may only be produced for a platform with a completed and independently
reviewed [validation report](VALIDATION_REPORT_TEMPLATE.md): exact CPU, BIOS and EC versions,
Windows build, Secure Boot and HVCI state, PCI MCHBAR evidence, three independent read-only
comparisons against HWiNFO and ThrottleStop, signature hashes, thermal and WHEA observations, a
rollback test and audit samples.

Windows loads the driver only with a valid signature. Do not disable Secure Boot, memory
integrity (HVCI), driver signature enforcement or the vulnerable-driver blocklist to load an
unsigned build.
