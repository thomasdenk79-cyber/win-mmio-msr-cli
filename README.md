# win-mmio-msr-cli

Windows-CLI in **Rust** zum Lesen, Berechnen und kontrollierten Setzen von Intel-RAPL-Package-Power-Limits (PL1/PL2) über **MSR + MMIO**. Zielplattform: Lenovo ThinkPad P16 Gen 2 mit Intel Core i9-13950HX.

> **Projektstatus: Rust-Kern und Simulation implementiert. Die echte Windows-Hardwareanbindung ist noch nicht verfügbar.** Sie benötigt einen lokal validierten, sicher implementierten und korrekt signierten Windows-Bridge-Treiber. Ein Treiber wird weder heruntergeladen noch mitgeliefert. Siehe [AGENTS.md](AGENTS.md).

## Warum zwei Registerpfade?

- **MSR 0x606:** Intel-RAPL-Einheiten (Watt und Sekunden).
- **MSR 0x610:** CPU-Package-PL1/PL2.
- **MCHBAR + 0x59A0:** MMIO-Package-Power-Limit-Register auf passenden Intel-Plattformen. Vor jedem Zugriff muss die tatsächliche physische Basisadresse und das Registerlayout auf dem betreffenden Rechner nachgewiesen werden.
- BIOS, EC, Intel DTT, Stromgrenzen und thermischer Schutz können die Einstellungen weiter einschränken oder überschreiben.

**Intel i9-13950HX:** Processor Base Power 55 W, Maximum Turbo Power 157 W. 157 W als PL2 ist nicht dasselbe wie eine dauerhaft sichere PL1-Grenze von 157 W.

## Bereits programmiert

- RAPL-Einheiten und 64-Bit-Power-Limit-Register dekodieren.
- PL1/PL2 per Read-Modify-Write berechnen. Zeitfenster, Clamp-/Lock-Bits und reservierte Bits bleiben unverändert.
- CPU-Profil und Bridge-Metadaten prüfen, unbekannte Rechner ablehnen.
- **status**, **plan** (read-only), **apply** (explizite Bestätigung).
- Backup vor einem Schreibversuch, Readback und best-effort Rollback bei Fehlern.
- Vollständige Simulation mit Unit-Tests unter Linux und Windows.
- Kein automatischer Treiberdownload, kein Abschalten von Windows-Sicherheitsfunktionen.

## Build

~~~powershell
cargo build --release
cargo test
.\target\release\win-mmio-msr-cli.exe help
~~~

Oder das mitgelieferte PowerShell-Skript: **scripts/build.ps1**.

## Sofort ausprobieren, OHNE Hardwarezugriff

~~~powershell
.\target\release\win-mmio-msr-cli.exe status --mock
.\target\release\win-mmio-msr-cli.exe plan --mock --pl1 55 --pl2 157
.\target\release\win-mmio-msr-cli.exe plan --mock --pl1 157 --pl2 157 --allow-sustained-above-base
~~~

Simulation eines Schreibvorgangs (ändert KEINE echten Register):

~~~powershell
.\target\release\win-mmio-msr-cli.exe apply --mock --pl1 55 --pl2 157 --backup "$env:TEMP\rapl-mock-before.txt" --yes
~~~

Die Backupdatei darf noch nicht existieren; das Programm überschreibt keine alte Backupdatei.

## Echte Windows-Nutzung NACH Abschluss der Hardwareanbindung

~~~powershell
.\target\release\win-mmio-msr-cli.exe status --bridge C:\Tools\rapl-bridge.exe
.\target\release\win-mmio-msr-cli.exe plan --bridge C:\Tools\rapl-bridge.exe --pl1 55 --pl2 157
.\target\release\win-mmio-msr-cli.exe apply --bridge C:\Tools\rapl-bridge.exe --pl1 55 --pl2 157 --backup C:\Temp\rapl-original.txt --yes
~~~

Das sind **Zielbefehle**; ein Bridge-EXE liegt aktuell noch nicht vor. Für PL1 >55 W ist zusätzlich **--allow-sustained-above-base** notwendig. Keine Auto-Reapply-Schleife, bevor Firmware- und Temperaturverhalten kontrolliert sind.

## Dokumentation für den lokalen Agenten

- [AGENTS.md](AGENTS.md): verbindliche Arbeitsregeln
- [Architektur](docs/ARCHITECTURE.md)
- [Bridge-Protokoll](docs/BRIDGE_PROTOCOL.md)
- [Handover](docs/HANDOFF.md)
- [Testplan](docs/TEST_PLAN.md)

## Referenzen

- Intel Package Power Limit, MCHBAR offset 59A0: https://edc.intel.com/content/www/ca/fr/design/publications/13th-generation-core-processor-datasheet-volume-2-of-2/package-power-limit-package-rapl-limit-0-0-0-mchbar-pcu-offset-59a0/
- Intel i9-13950HX Spezifikationen: https://www.intel.de/content/www/de/de/products/sku/232149/intel-core-i913950hx-processor-36m-cache-up-to-5-50-ghz/specifications.html
- Microsoft Driver Security Best Practices: https://learn.microsoft.com/en-us/windows-hardware/drivers/driversecurity/driver-security-dev-best-practices

Das Projekt entfernt oder umgeht keinen thermischen Schutzmechanismus.

## CLI 0.2: Bedienung und Automatisierung

Die CLI validiert alle Schreibparameter vor dem Start der Bridge, beendet eine nicht antwortende Bridge nach `--timeout-ms`, begrenzt Bridge-Ausgaben und unterstützt farbige Terminalansichten (`--color auto|always|never`) sowie maschinenlesbares JSON (`--json`).

```powershell
# Diagnose und Planung ohne Hardware
win-mmio-msr-cli status --mock --color always
win-mmio-msr-cli plan --mock --preset base
win-mmio-msr-cli plan --mock --preset turbo --allow-sustained-above-base --json

# Livebetrieb erst nach Bridge- und Messvalidierung
win-mmio-msr-cli status --bridge C:\Tools\rapl-bridge.exe --timeout-ms 10000
win-mmio-msr-cli apply --bridge C:\Tools\rapl-bridge.exe --preset base --backup C:\Temp\rapl-before.txt --yes

# PowerShell-Vervollständigung
win-mmio-msr-cli completions powershell > $PROFILE.CurrentUserAllHosts
```

JSON besitzt `schema_version: 1`, `ok`, `command` und strukturierte Rohwerte/decoded limits. `apply` erstellt ausschließlich neue Backups und führt nach jedem Schreibvorgang einen exakten Readback durch.
