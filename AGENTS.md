# AGENTS.md: Handover an den lokalen Windows-Agenten

## Ziel

**Die bestehende Rust-CLI fertigstellen, nicht neu schreiben.** Der Kern in src/ berechnet Intel RAPL Power Limits, erstellt Snapshots/Backups, prüft Readback und bietet best-effort Rollback. Der sichere Windows-Hardwarezugriff ist der nächste Arbeitsschritt.

**Zielhardware:** Lenovo ThinkPad P16 Gen 2, Intel Core i9-13950HX, Windows 11. Bekannte Probleme: intermittierende 25-W-PL1-Begrenzung und auffällig hohe Temperaturen von Board/SSD/RAM. Ziel ist Diagnose und kontrollierte MSR-/MMIO-Konfiguration, vergleichbar mit ThrottleStop Sync MMIO.

## Verbindliche Sicherheitsregeln

1. Keine frei adressierbaren MSR-, PCI- oder physischen Speicher-Schreibbefehle über CLI oder Kernel-API.
2. Kein RwDrv/WinRing0 oder anderer blockierter Universaltreiber. HVCI/Memory Integrity, Signaturzwang und Vulnerable Driver Blocklist NICHT deaktivieren.
3. Im Kernel NUR die notwendigen CPU-Register freigeben: MSR 0x606 READ, 0x610 READ/WRITE; MMIO ausschließlich MCHBAR+0x59A0 im verifizierten Intel-Profil.
4. MCHBAR-Basis nicht als fixe physische Adresse aus Beispielcode kopieren. Vorher PCI-Basis, Enable-Bit, gültigen BAR-Bereich, Offset, 32/64-Bit-Layout und Lock-Bits nachweisen.
5. Treiberzugriff nur Administrator/SYSTEM; ungültige IOCTLs, Datenlängen, CPUs, PCI-Geräte und Werte ablehnen.
6. Nie Lock-/Clamp-/Zeitfenster-/Reserved-Bits ohne separate technische Freigabe verändern. Vorher Snapshot, persistentes Backup und Nachher-Readback.
7. Bei Übertemperatur, WHEA, EC-Alarmen, Treiberproblemen und unerwartetem Firmware-Override STOPPEN, nicht mit enger Polling-Schleife dagegen schreiben.
8. 157 W ist Max Turbo, nicht freigegebener dauerhafter PL1. 157-W-PL1-Belastung gehört NICHT in die automatische Testmatrix.

## Bereits umgesetzt

- **src/rapl.rs:** RAPL-Units, limitierte RMW-Maske, Decoder und Unit-Tests
- **src/backend.rs:** HardwareBackend, MockBackend, sichere Prozess-Bridge-Schnittstelle
- **src/lib.rs:** CPU-Profilcheck, Snapshot, Plan, Backup, Apply, Readback, Rollback
- **src/main.rs:** CLI status / plan / apply, Opt-in-Flags
- **docs/BRIDGE_PROTOCOL.md:** konkreter v1-Vertrag

## Arbeitspakete

### A. Build und statische Tests

- Rust stable auf Windows x64 (MSVC), cargo test --all-targets, cargo build --release.
- Compiler-/Lint-Probleme beheben, cargo fmt, cargo clippy -- -D warnings.
- Zusätzliche Tests: Firmware-Races, Lockbits, Backup-Fehler, Rollback, ungültige Wattwerte, Konkurrenz mehrerer Bridge-Prozesse.

### B. CPU und MMIO-Adressen verifizieren

- CPU CPUID, Chipsatz, BIOS, Windows-Build, Secure Boot, HVCI/VBS, CPU-Core-/Gruppenaffinität aufnehmen.
- Offizielle Intel-Raptor-Lake-HX-Unterlagen auf MCHBAR und Package-RAPL-Limit beziehen.
- Die tatsächliche MCHBAR-Basis aus PCI-Konfiguration korrekt ermitteln; MMIO-Offset 0x59A0 und 64-Bit-Layout bestätigen.
- Read-only drei unabhängige Vergleichsmessungen: MSR 0x606, MSR 0x610, MCHBAR 0x59A0/0x59A4, ThrottleStop/HWiNFO. Nur bei validierter Übereinstimmung mmio_59a0_64bit_verified=true zurückgeben.
- Ursachen des 25-W-Firmware-Limits dokumentieren statt sie zu übersteuern.

### C. Windows-Bridge IMPLEMENTIEREN (noch offen)

- Separates rapl-bridge.exe mit dem v1-Protokoll unter docs/BRIDGE_PROTOCOL.md implementieren.
- Eng eingeschränkten, auditierbaren, korrekt signierten KMDF/WDM-Treiber oder vergleichbaren genehmigten Windows-Zugriff entwickeln. **Keine** ungesicherte universelle IOCTL-/MSR-/MMIO-Schnittstelle.
- ACL/SDDL, IOCTL-Validierung, CPU-Affinität, Speicher-Mapping-Lifetime, PCI-Erkennung und Datenbreite nachweisen.
- Treibersignatur, WDK-Build, INF und Prüfsummen dokumentieren. Ohne Signatur gilt Livebetrieb als BLOCKIERT.
- Erst Read-only-Kommandos info/msr-read/mmio-read. Nach unabhängiger Verifikation Schreibkommandos hinzufügen und Readback implementieren.
- Keine Binary-Treiber von Drittprojekten ins Repository übernehmen.

### D. Tests auf echter Hardware

- Phase 1: Mock und Windows Build.
- Phase 2: Read-only Bridge; alle Rohwerte unabhängig verifizieren.
- Phase 3: konservative Konfiguration PL1=55 W, PL2=157 W, Backups und direkte Readbacks, mit Monitoring.
- Fehler-Simulation, Suspend/Resume, Reboot, BIOS-/DTT-Override, Admin-/Nicht-Admin, CPU-Affinität untersuchen.
- CPU/Board/PCH/SSD/RAM und WHEA protokollieren.
- **Kein** Dienst, Autostart, Watchdog oder automatisches Reapply ohne ausdrückliche Freigabe.

### E. Ergebnis und Abnahme

- CI grün, dokumentierter x64-Releasebuild, Quellcode aller Komponenten, Release-ZIP mit SHA256.
- docs/VALIDATION_REPORT.md anlegen: Hardware, Rohwerte, Messvergleich, Tests, signierter Treiberstatus, Einschränkungen und Risiken.
- Nicht hardware-fertig melden, solange Treiber oder MMIO-Verifikation fehlen.

## Abnahmekriterien

Mock funktioniert ohne Treiber; unsupported CPUs/Lockbits/unpassende MMIO-Basis brechen ab; MSR/MMIO-Limits bleiben auf den beiden Pfaden synchron; alle absichtlich geänderten Bits werden korrekt zurückgelesen; Fehlschläge liefern verständliche Fehler/Backup/Rollback-Status; keine Windows-Sicherheitsmaßnahme muss abgeschaltet werden.

Primärdokumentation unter docs/ und README. Technische Änderungen mit nachprüfbarer Quelle begründen.
