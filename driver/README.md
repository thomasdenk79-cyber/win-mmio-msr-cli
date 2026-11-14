# `rapl-pkg.sys`

KMDF non-PnP driver that exposes the control device `\\.\RaplPkg` to `rapl-bridge.exe`.
It is not validated on hardware and is not signed; see [Signing](#signing).

## Design

- Control device `\Device\RaplPkg` with the SDDL `D:P(A;;GA;;;SY)(A;;GA;;;BA)` (SYSTEM and
  Administrators only) and `FILE_DEVICE_SECURE_OPEN`.
- Fixed register whitelist: MSR `0x606` (read), MSR `0x610` (read, write) and
  `MCHBAR+0x59A0` (read, write). Callers never pass MSR numbers or physical addresses.
- Protocol 2 (`include/rapl_ioctl.h`): `METHOD_BUFFERED` IOCTLs with exact buffer sizes
  (request 32, response 24, info 40 bytes). `bridge/src/abi.rs` mirrors the header; the bridge
  test `driver_header_matches_the_rust_contract` fails when the two drift apart.
- CPU gate, evaluated once at load: GenuineIntel, family 6, model `0xB7`, exactly one package.
  Every register IOCTL fails with `STATUS_NOT_SUPPORTED` on other systems.
- Writes are a compare-and-write: the driver reads the current value, refuses with
  `STATUS_RETRY` when it differs from the bridge's expected value, applies the write policy,
  writes, reads back and returns the readback. The bridge audits a readback mismatch.
- The write policy repeats `check_register_write` from `core/src/policy.rs` independently of the
  bridge: lock bit clear, only the PL1/PL2 power and enable bits change, both limits stay
  enabled and `5 W <= PL1 <= PL2 <= 157 W` in the CPU's power unit (integer arithmetic).
- `rdmsr`/`wrmsr` faults (for example an access refused by the hypervisor) are caught and
  reported as `STATUS_IO_DEVICE_ERROR`.

## Build-time gates

| MSBuild property | C macro | Default | Effect |
|---|---|---|---|
| `RaplEnableMmioRead` | `RAPL_ENABLE_MMIO_READ` | 0 | Reads MCHBAR from host bridge 0:0.0 (offset `0x48`) and allows `READ_MMIO` |
| `RaplMmioLayoutValidated` | `RAPL_MMIO_LAYOUT_VALIDATED` | 0 | Reports `MMIO_VERIFIED`; with live writes also allows `WRITE_MMIO`. Requires `RaplEnableMmioRead=1` |
| `RaplEnableLiveWrites` | `RAPL_ENABLE_LIVE_WRITES` | 0 | Clears `READ_ONLY` and allows `WRITE_MSR` |

The default build only reads MSR `0x606` and `0x610`. Enable a gate only in a reviewed build for
a platform whose validation report (`docs/VALIDATION_REPORT_TEMPLATE.md`) covers it.

## Build

Requirements: Visual Studio 2022 with the C++ workload and the MSVC Spectre-mitigated
libraries, the Windows SDK and the WDK 10.0.26100 with its Visual Studio extension.

```powershell
.\scripts\build.ps1 -Driver
# or directly with the 64-bit MSBuild (MSBuild\Current\Bin\amd64\MSBuild.exe):
msbuild driver\rapl-pkg.vcxproj -p:Configuration=Release -p:Platform=x64
# gates, for example: -p:RaplEnableLiveWrites=1
```

Use the 64-bit MSBuild: the WDK 26100 build tasks load InfVerif and ApiValidator for the MSBuild
process architecture and ship no x86 `InfVerif.dll`, so the 32-bit MSBuild fails INF verification.
`scripts\build.ps1 -Driver` finds the 64-bit MSBuild with `vswhere`.

The package (`rapl-pkg.sys`, the stamped `rapl-pkg.inf` and the unsigned `rapl-pkg.cat`) is
written to `target\driver\x64\Release\rapl-pkg\`; the PDB stays in `target\driver\x64\Release\`.
The CI job `driver-windows` builds the default package with warnings as errors (including
ApiValidator), compiles once with every gate enabled (never shipped), runs code analysis with the
Driver Recommended Rules on that build, checks the package with
`.github/scripts/driver-package-check.ps1` (InfVerif, `DriverVer`, x64 image, signature status)
and uploads the artifact `driver-package-x64-unsigned`.

## Signing

Windows loads a kernel driver only with a valid signature. On a system with Secure Boot and
memory integrity (HVCI) this requires a Microsoft signature, for example attestation signing
through the Partner Center with an EV code signing certificate. This project does not provide a
signed driver. Do not disable Secure Boot, memory integrity or driver signature enforcement to
load an unsigned build.

## Install and remove (signed and validated builds only)

```powershell
pnputil /add-driver rapl-pkg.inf /install
sc.exe start rapl-pkg
.\rapl-bridge.exe info

sc.exe stop rapl-pkg
pnputil /enum-drivers                       # find the oemNN.inf of rapl-pkg.inf
pnputil /delete-driver oemNN.inf /uninstall
```

The INF is a primitive driver package (`DefaultInstall`, no hardware ID). The service
`rapl-pkg` starts on demand only.

## Error codes

| NTSTATUS | Win32 | Meaning |
|---|---|---|
| `STATUS_INVALID_DEVICE_REQUEST` | 1 | Unknown IOCTL |
| `STATUS_ACCESS_DENIED` | 5 | Caller is not SYSTEM or an Administrator, or the handle lacks write access |
| `STATUS_NOT_SUPPORTED` | 50 | Unsupported CPU, MCHBAR unavailable or build-time gate disabled |
| `STATUS_INVALID_PARAMETER` | 87 | Register not valid for the IOCTL, non-zero reserved or read fields, or write policy violation |
| `STATUS_IO_DEVICE_ERROR` | 1117 | The CPU or hypervisor refused the MSR access |
| `STATUS_RETRY` | 1237 | The register changed since the bridge read it |
| `STATUS_REVISION_MISMATCH` | 1306 | Buffer size or protocol version differs from protocol 2 |
| `STATUS_INSUFFICIENT_RESOURCES` | 1450 | Mapping `MCHBAR+0x59A0` failed |

`rapl-bridge` explains these codes in its error messages.

## Validation status

CI proves that the driver compiles with warnings as errors, passes code analysis and ApiValidator,
that the INF passes InfVerif and that the ABI matches the bridge. It does not prove behaviour on
hardware. Loading the driver requires a signed build and a completed validation report.
