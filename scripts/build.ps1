$ErrorActionPreference = 'Stop'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust/Cargo fehlt: https://rustup.rs/"
}
cargo test --all-targets
if ($LASTEXITCODE -ne 0) { throw "Tests fehlgeschlagen" }
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "Release-Build fehlgeschlagen" }
Write-Host "Erfolgreich: target\release\win-mmio-msr-cli.exe"
