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

## Übergabe nach CLI-Review (v0.2)

Die lokale Agentenarbeit beginnt jetzt beim Windows-Hardwareteil. Die CLI ist in `src/main.rs`, `src/output.rs`, `src/backend.rs` und `src/lib.rs` für folgende Punkte gehärtet:

- Clap-Unterbefehle und gegenseitig ausschließende `--mock`/`--bridge`-Auswahl.
- Presets `base` (55/157 W) und `turbo` (157/157 W) sowie Pflicht-Opt-in für PL1 über 55 W.
- `--json`, `--color`, `--timeout-ms` und PowerShell-Vervollständigung.
- Bridge-Prozess mit begrenzten stdout/stderr-Pipes, Timeout, Kill bei Hänger und Prüfung von Protokollfeldern.
- Rollback liest vor dem Zurückschreiben erneut und überspringt eine Wiederherstellung bei unerwarteten Registeränderungen; danach wird zurückgelesen.

### Lokale Testreihenfolge

```powershell
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo build --release
.\target\release\win-mmio-msr-cli.exe status --mock --color always
.\target\release\win-mmio-msr-cli.exe plan --mock --preset base --json
.\target\release\win-mmio-msr-cli.exe apply --mock --preset base --backup "$env:TEMP\rapl-v02.txt" --yes
```

Danach die Bridge gemäß `docs/BRIDGE_PROTOCOL.md` implementieren: zuerst read-only, Windows x64/MSVC, signierter enger Treiber, PCI-MCHBAR-Basis und 0x59A0/0x59A4 unabhängig verifizieren. Erst wenn `info` `mmio_59a0_64bit_verified=true` melden darf, soll die CLI live schreiben. Für den lokalen Agenten sind echte Messwerte, Signaturstatus und ein `docs/VALIDATION_REPORT.md` verpflichtend.
