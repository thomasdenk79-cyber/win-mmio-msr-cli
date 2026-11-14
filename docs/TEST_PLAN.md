# Testplan

| Phase | Ort | Vorgang | Erwartung |
| --- | --- | --- | --- |
| 0 | CI Linux + Windows | cargo test --all-targets | Mock-/RAPL-Tests grün |
| 1 | Windows ohne Driver | status --mock, plan --mock | konsistente Rohwerte, keinerlei Hardwareänderung |
| 2 | Windows ohne Driver | CLI ohne --bridge | verweigert Zugriff |
| 3 | Windows + Bridge, read-only | info, msr-read, mmio-read | echte CPU/MCHBAR, korrekte Rohwerte |
| 4 | Windows + Bridge | status und plan | Vergleich mit ThrottleStop/HWiNFO erfolgreich |
| 5 | Windows + Bridge | unbekanntes CPU-Modell, Lockbit, falsche Adresse | Abbruch VOR Schreiben |
| 6 | Windows + Bridge | konservativer apply | exklusives Backup, verifiziertes Readback |
| 7 | Windows + Bridge | simulierte Treiberfehler | Rollback oder klare Fehlermeldung |
| 8 | Windows + Bridge | Suspend/Resume/Firmware-Override | Verhalten dokumentiert, keine Reapply-Schleife |

## Messwerte

CPU, BIOS/EC, Chipsatz, Windows-Build, Secure Boot und HVCI/VBS, MSR 0x606/0x610 hex, PCI-MCHBAR und MMIO 0x59A0/0x59A4 hex, PL1/PL2 und Bitflags, Temperatur CPU-Package/PCH/Board/RAM/SSD/VRM, WHEA und Firmware-Override.

## Stop-Kriterien

Unbestätigte MMIO-Adresse, Treiber durch Windows blockiert, nötige Sicherheits-Deaktivierung, inkonsistente Leseergebnisse, EC-/Thermal-Alarm, auffällige Temperaturen, WHEA, fehlgeschlagener Rollback.

**Dauerhaft PL1=157 W ist kein automatischer Test.** Ein erfolgreicher Register-Readback ist kein thermischer Freibrief.
