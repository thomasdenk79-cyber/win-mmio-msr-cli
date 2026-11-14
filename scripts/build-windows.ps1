$ErrorActionPreference = 'Stop'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Rust/Cargo fehlt.' }
cargo test --all-targets
cargo build --release
cargo test --manifest-path bridge/Cargo.toml
cargo build --manifest-path bridge/Cargo.toml --release --target x86_64-pc-windows-msvc
Write-Host 'Rust-CLI und Bridge gebaut. Der KMDF-Treiber benötigt WDK/MSBuild und eine Signatur; siehe driver/README.md.'
