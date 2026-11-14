# Architektur

~~~text
                  win-mmio-msr-cli.exe
                   | Intel-CPU Profil-Gate
                   | RAPL-Einheiten + RMW
                   | Snapshot / Plan / Backup
                   | Readback / Rollback
                   |
               HardwareBackend trait
                 /          \
           MockBackend    ExternalBridge (Subprozess v1)
          (implementiert)       |
                             rapl-bridge.exe
                            (NOCH OFFEN)
                                |
                          eng eingeschränkter,
                          signierter Windows-Treiber
                                |
                          MSR 0x606 [R]
                          MSR 0x610 [R/W]
                          MCHBAR+0x59A0 [R/W]
~~~

## Datenmodell

- MSR 0x606: RAPL-Leistungseinheit 2^(-power_unit) W je Einheit; Zeiteinheit 2^(-time_unit) Sekunden.
- MSR 0x610: PL1 bits 14:0, enable 15, clamp 16, time 23:17; PL2 bits 46:32, enable 47, clamp 48, time 55:49; lock bit 63.
- Für MMIO wird dieselbe 64-Bit-Maske nur verwendet, wenn die Bridge das tatsächliche MCHBAR-Registerlayout auf diesem Intel-HX-Gerät geprüft hat.
- RMW ändert nur die beiden 15-Bit-Leistungswerte und Enable-Bits; alle anderen Bits bleiben erhalten.

## Kontrollierter Schreibablauf

1. CPU-/Plattformidentität prüfen, MSR-Einheiten und beide Raw-Register einlesen.
2. Wattwerte 5 <= PL1 <= PL2 <= 157 prüfen. Über 55 W PL1 ist zusätzliches Opt-in nötig.
3. Plan berechnen, beide Originalregister und Zielregister ausgeben, zunächst nichts schreiben.
4. Unmittelbar vor Apply erneut lesen; bei zwischenzeitlicher Firmwareänderung abbrechen.
5. Exklusives Backupdatei-Neuanlegen und Synchronisieren.
6. MMIO schreiben, danach MSR. Reihenfolge ist vom lokalen Agenten plattformspezifisch zu prüfen.
7. Beide Register erneut lesen; die absichtlich modifizierten Felder müssen exakt übereinstimmen.
8. Bei Fehler MSR und MMIO bestmöglich zurückschreiben; expliziten Rollback-Status ausgeben.

## Grenzen

- MMIO- und MSR-Schreiben ist keine atomare Transaktion; bei Treiberausfall kann auch der Rollback scheitern.
- BIOS/EC/DTT, VRM-Limits und Temperaturschutz bleiben wirksam.
- Keine BIOS-Variablen, TCC-/PROCHOT-Änderungen, PL4-/EC-Manipulationen, kein dauerhaftes Erzwingen.
- Keine Hintergrunddienste, kein Autostart, kein Watchdog.
