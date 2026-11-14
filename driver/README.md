# `rapl-pkg.sys`

Der Treiber ist ein KMDF/WDK-Projektgerüst. Er wird absichtlich nicht mit MinGW oder einem generischen User-Mode-Treiber gebaut.

## Build unter Windows

1. Visual Studio 2022 mit Windows Driver Kit installieren.
2. Eine `x64 Native Tools Command Prompt for VS 2022` als Administrator öffnen.
3. `msbuild`/WDK-KMDF-Vorlagen verwenden und `driver/src/driver.c`, `driver/include/rapl_ioctl.h` sowie `driver/inf/rapl-pkg.inf` einbinden.
4. `RAPL_ENABLE_LIVE_WRITES=0` beibehalten.
5. `.sys`, `.inf` und `.cat` mit einer gültigen Test- oder Produktionssignatur versehen.
6. Vor Installation `signtool verify /kp /c rapl-pkg.cat rapl-pkg.sys` ausführen.

Ohne gültige Signatur, nachvollziehbare PCI-MCHBAR-Prüfung und dokumentierte Read-only-Vergleiche darf der Treiber nicht installiert werden. Secure Boot, HVCI und die Vulnerable Driver Blocklist bleiben aktiv.

Der eingecheckte Treiber kann MSR 0x606/0x610 lesen, meldet MMIO nicht verifiziert und verweigert jede Schreiboperation mit `STATUS_NOT_SUPPORTED`. Das ist der beabsichtigte sichere Ausgangszustand.
