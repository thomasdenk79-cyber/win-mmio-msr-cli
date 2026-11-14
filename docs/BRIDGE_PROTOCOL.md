# Windows Hardware Bridge Protocol v1

Die Rust-CLI ist für Policy, mathematische Umrechnung, Snapshot, Backup und Readback zuständig; die Bridge nur für streng eingeschränkte privilegierte Operationen. **Aktuell wird keine Bridge-Datei ausgeliefert.**

## Prozessvertrag

Die CLI ruft eine lokal ausdrücklich angegebene EXE mit std::process::Command auf, ohne Shell. Jede Invocation ist synchron.

### 1. Hardwareidentität

~~~text
rapl-bridge.exe info
~~~

Ausgabe auf stdout (UTF-8), eine Zeile je key=value:

~~~text
protocol=1
vendor=GenuineIntel
model=Intel(R) Core(TM) i9-13950HX
mmio_59a0_64bit_verified=true
~~~

Die Bridge darf verified=true NUR melden, wenn CPU/CPUID und Chipsatz stimmen, PCI-MCHBAR-Basis und Enable-Bit korrekt erkannt sind, das 64-Bit-Layout verifiziert ist und mindestens ein unabhängiger Vergleichswert zur echten Hardware vorliegt. Nicht eine feste physische Adresse blind übernehmen.

### 2. Leseoperationen

~~~text
rapl-bridge.exe msr-read 0x606
rapl-bridge.exe msr-read 0x610
rapl-bridge.exe mmio-read 0x59a0
~~~

stdout enthält genau EINEN Hex-Wert mit Präfix 0x, 64 Bit unsigned. Beispiel: 0x00000000000a1003. Alle Diagnosemeldungen gehören auf stderr. MMIO liefert den vollständigen 64-Bit-Wert aus den 32-Bit-Hälften +0x59A0/+0x59A4, soweit die offizielle Plattformdokumentation dies bestätigt.

### 3. Schreiboperationen

~~~text
rapl-bridge.exe msr-write 0x610 0x0123456789abcdef
rapl-bridge.exe mmio-write 0x59a0 0x0123456789abcdef
~~~

**Nur Protokollbeispiele, keine Registerwerte zum direkten Ausführen.** Die Bridge MUSS unerlaubte Werte abweisen, insbesondere wenn sie andere als PL1/PL2-Numeric- und Enable-Bits ändern würden. Unmittelbar vor Schreiben aktuelles Register einlesen und auf konkurrierende Änderungen prüfen. Erfolgreicher Write mit Readback muss auf stdout exakt OK ausgeben und Exit-Code 0 liefern; alle Fehler nonzero auf stderr.

### Sicherheitsanforderungen

- Kernel-seitig whitelist: MSR 0x606 nur Lesen, 0x610 Lesen/Schreiben, MMIO ausschließlich validiertes MCHBAR-Register 0x59A0.
- Kein generisches rdmsr/wrmsr, kein Port-I/O und kein frei adressierbarer physischer Speicher.
- Zugriff nur Administrator/SYSTEM; keine Abschaltung von HVCI, Blocklist oder Signatur.
- CPU-/Prozessorgruppen-Affinität und Package-Semantik prüfen.
- Ein systemweites Lock/Mutex gegen zwei gleichzeitige Bridge-Prozesse implementieren.
- 64-Bit-MMIO-Zugriff kann plattformspezifisch zwei 32-Bit-Operationen erfordern. Richtige Reihenfolge belegen, nicht atomare Writes behaupten.
- Nicht erkannte CPU, gesperrte Register, veränderte Firmware-Werte oder fehlerhaftes MMIO-Mapping => Fehler.
- stdout höchstens 4096 Bytes; kein auto-download oder Einfügen fremder Ring-0-Treiber.

## Stand

Der Adapter und Kernel-Treiber fehlen noch. Dieses bewusst offene Hardware-Gate ist in AGENTS.md und docs/HANDOFF.md dem lokalen Agenten zugewiesen.
