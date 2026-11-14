# `rapl-pkg.sys`

This is a KMDF/WDK driver skeleton. Build it with Visual Studio and the Windows Driver Kit, never with MinGW or a generic user-mode driver.

## Windows build

1. Install Visual Studio 2022 and the Windows Driver Kit.
2. Open an x64 Native Tools command prompt with WDK environment variables.
3. Build `driver/src/driver.c` with `driver/include/rapl_ioctl.h` and package `driver/inf/rapl-pkg.inf`.
4. Keep `RAPL_ENABLE_LIVE_WRITES=0` for validation builds.
5. Generate the catalog and apply a valid test or production signature.
6. Verify with `signtool verify /kp /c rapl-pkg.cat rapl-pkg.sys`.

The checked-in driver reads MSR 0x606/0x610, reports MMIO as unverified, and rejects every write with `STATUS_NOT_SUPPORTED`. This is the deliberate safe starting point. Do not install it until the WDK build, signature and validation report are complete.
