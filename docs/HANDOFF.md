# Handover: lokaler Windows-Agent

## Stand

Die Rust-CLI ist bereits programmiert und läuft im Mock ohne Privilegien. Die privilegierte Windows-Bridge existiert noch nicht. Echte Änderungen an 157-W-Einstellungen sind deshalb aktuell absichtlich unmöglich.

## Einstieg

~~~powershell
git clone https://github.com/thomasdenk79-cyber/win-mmio-msr-cli.git
cd win-mmio-msr-cli
cargo test --all-targets
cargo build --release
.\target\release\win-mmio-msr-cli.exe status --mock
.\target\release\win-mmio-msr-cli.exe plan --mock --pl1 55 --pl2 157
~~~

Falls Featurebranch noch nicht gemergt: feat/rust-rapl-cli auschecken.

## Arbeitsaufträge

1. Rust stable für x86_64-pc-windows-msvc verifizieren, Tests/Formatter/Clippy ausführen und Compilerfehler beheben.
2. Die für Lenovo P16 Gen 2 gültige PCI-MCHBAR-Basis, Intel-Datenblatt und MMIO-Register 0x59A0/0x59A4 prüfen.
3. ThrottleStop/HWiNFO als read-only Baseline verwenden; rohe MSR- und MMIO-Werte mehrfach erfassen, BIOS/DTT-Override dokumentieren.
4. Separates rapl-bridge.exe und korrekt signierten, engen Kernel-Treiber gemäß BRIDGE_PROTOCOL.md programmieren. Zunächst nur Lesen/Info.
5. Treiber-ACL/IOCTL-Validierung, CPU-Affinität, 64-Bit-Teilregister, Race Conditions und Fehlerbehandlung testen.
6. Erst danach kontrolliert schreiben und mit Backup, Readback und Monitoring verifizieren (konservativ PL1=55, PL2=157 W).
7. docs/VALIDATION_REPORT.md mit CPU/BIOS/Versionen, vorher/nachher Rohwerten, Temperaturdaten und WHEA-Status anlegen.
8. Windows-x64-Release-ZIP mit SHA256 bereitstellen; Treibersignatur und bekannte Grenzen offen dokumentieren.

## Niemals

- Unsichere Universaltreiber einsetzen, um Windows-Schutzfunktionen abzuschalten.
- 157 W dauerhaft ohne thermische Validierung als PL1 erzwingen.
- Einen CPU-, PCI- oder MMIO-Check mit angenommener Adresse überspringen.
- Hardware-Fertigstellung melden, solange Bridge, Signierung und reale Tests fehlen.
