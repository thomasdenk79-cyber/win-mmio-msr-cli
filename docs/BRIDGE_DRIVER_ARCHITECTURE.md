# Bridge- und Treiberarchitektur

## Ziel

`win-mmio-msr-cli.exe` bleibt die Policy-Schicht. Die privilegierte Hardware-Schicht besteht aus zwei getrennten Komponenten:

1. `rapl-bridge.exe`: kleiner Windows-Prozess ohne eigene Registerlogik. Er validiert das v1-Protokoll, öffnet ausschließlich `\\.\RaplPkg`, serialisiert Requests und gibt deterministische Antworten aus.
2. `rapl-pkg.sys`: enger KMDF-Treiber mit einer festen IOCTL-Whitelist. Er darf ausschließlich die drei explizit freigegebenen Register adressieren.

```text
CLI -> rapl-bridge.exe -> \Device\RaplPkg -> rapl-pkg.sys -> MSR / validiertes MCHBAR
```

## Sicherheitsgates

- Der Treiber wird mit `RAPL_ENABLE_LIVE_WRITES=0` gebaut. In diesem Modus sind MSR/MMIO-Schreib-IOCTLs absichtlich `STATUS_NOT_SUPPORTED`.
- `info` meldet `mmio_59a0_64bit_verified=true` erst nach CPU-, Chipsatz-, PCI-BAR- und unabhängiger Read-only-Prüfung.
- Es gibt keine frei adressierbaren Adressen: Register-ID ist eine Enumeration, keine physische Adresse.
- Jede IOCTL prüft Version, Strukturgröße, Register-ID, CPU/Package, Reserved-Bits und Schreibmaske.
- MMIO-Mapping ist nur während einer Operation gültig und wird danach aufgehoben. Eine feste MCHBAR-Adresse ist verboten.
- Der Gerätetag ist nur für Administratoren und SYSTEM zugänglich; kein User-Mode-Schreibzugriff.

## Zustandsautomat

```text
UNVERIFIED -> READ_ONLY_VERIFIED -> WRITE_ENABLED (separater Build + Review)
     |               |                      |
     +-- Fehler -----+----------- STOP <----+
```

`WRITE_ENABLED` wird in diesem Repository nicht automatisch erzeugt. Vorher müssen Intel-Datenblatt, konkrete PCI-Konfiguration, drei Vergleichsmessungen, Signatur und reale Rollback-Tests in `docs/VALIDATION_REPORT.md` dokumentiert sein.

## IOCTL-Vertrag

Der gemeinsame Vertrag liegt in `driver/include/rapl_ioctl.h`. Der Bridge-Prozess verwendet nur:

- `IOCTL_RAPL_GET_INFO`
- `IOCTL_RAPL_READ_MSR`
- `IOCTL_RAPL_READ_MMIO`
- `IOCTL_RAPL_WRITE_MSR` und `IOCTL_RAPL_WRITE_MMIO` erst nach dem Build-Gate

Der Treiber akzeptiert keine Roh-IOCTL mit beliebigen MSR-Nummern oder physischen Adressen.

## Build und Signatur

Der lokale Windows-Agent baut mit Visual Studio/WDK und erzeugt `.sys`, `.inf` und `.cat`. Ohne gültige Test-/Produktionssignatur gilt Livebetrieb als blockiert. HVCI, Secure Boot und die Vulnerable Driver Blocklist werden nicht abgeschaltet.
