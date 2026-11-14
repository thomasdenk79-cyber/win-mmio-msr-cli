//! Hardware access facade. No kernel drivers or unsigned binaries are bundled.
//! Real hardware I/O is delegated to a separately audited, local Windows bridge.
use std::path::PathBuf;
use std::process::Command;
use crate::rapl::{with_power_limits, Units};

#[derive(Debug, Clone)]
pub struct Identity {
    pub vendor: String,
    pub model: String,
    pub mmio_verified: bool,
    pub simulated: bool,
}

pub trait HardwareBackend {
    fn identity(&mut self) -> Result<Identity, String>;
    fn read_units(&mut self) -> Result<u64, String>;
    fn read_msr_limits(&mut self) -> Result<u64, String>;
    fn read_mmio_limits(&mut self) -> Result<u64, String>;
    fn write_msr_limits(&mut self, value: u64) -> Result<(), String>;
    fn write_mmio_limits(&mut self, value: u64) -> Result<(), String>;
}

pub struct ExternalBridge {
    path: PathBuf,
}

impl ExternalBridge {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let full = path.canonicalize()
            .map_err(|e| format!("Bridge executable cannot be resolved: {e}"))?;
        if !full.is_file() || full.extension()
            .and_then(|e| e.to_str())
            .map(|e| !e.eq_ignore_ascii_case("exe"))
            .unwrap_or(true) {
            return Err("Bridge must point to an existing .exe".into());
        }
        Ok(Self { path: full })
    }

    fn call(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(&self.path).args(args).output()
            .map_err(|e| format!("Cannot execute hardware bridge: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Bridge returned {}: {}", output.status, err.trim()));
        }
        if output.stdout.len() > 4096 {
            return Err("Bridge response exceeds 4096 bytes".into());
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| "Bridge response is not UTF-8")?;
        Ok(text.trim().to_string())
    }

    fn read_hex(&self, args: &[&str]) -> Result<u64, String> {
        let output = self.call(args)?;
        let value = output.strip_prefix("0x").or_else(|| output.strip_prefix("0X"))
            .ok_or("Bridge read output must be a single 0x-prefixed hex value")?;
        u64::from_str_radix(value, 16)
            .map_err(|_| format!("Invalid hexadecimal bridge value: {output}"))
    }

    fn write_hex(&self, kind: &str, address: &str, value: u64) -> Result<(), String> {
        let hex = format!("0x{value:016x}");
        let result = self.call(&[kind, address, &hex])?;
        if result != "OK" {
            return Err(format!("Bridge write did not acknowledge OK: {result}"));
        }
        Ok(())
    }
}

impl HardwareBackend for ExternalBridge {
    fn identity(&mut self) -> Result<Identity, String> {
        let result = self.call(&["info"])?;
        let mut vendor = None;
        let mut model = None;
        let mut verified = false;
        let mut protocol = false;
        for line in result.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "protocol" if v.trim() == "1" => protocol = true,
                    "vendor" => vendor = Some(v.trim().to_string()),
                    "model" => model = Some(v.trim().to_string()),
                    "mmio_59a0_64bit_verified" if v.trim() == "true" => verified = true,
                    _ => {}
                }
            }
        }
        if !protocol { return Err("Unsupported bridge protocol (expected protocol=1)".into()); }
        Ok(Identity {
            vendor: vendor.ok_or("Bridge did not provide vendor")?,
            model: model.ok_or("Bridge did not provide model")?,
            mmio_verified: verified,
            simulated: false,
        })
    }

    fn read_units(&mut self) -> Result<u64, String> { self.read_hex(&["msr-read", "0x606"]) }
    fn read_msr_limits(&mut self) -> Result<u64, String> { self.read_hex(&["msr-read", "0x610"]) }
    fn read_mmio_limits(&mut self) -> Result<u64, String> { self.read_hex(&["mmio-read", "0x59a0"]) }
    fn write_msr_limits(&mut self, value: u64) -> Result<(), String> {
        self.write_hex("msr-write", "0x610", value)
    }
    fn write_mmio_limits(&mut self, value: u64) -> Result<(), String> {
        self.write_hex("mmio-write", "0x59a0", value)
    }
}

/// In-memory test backend. NEVER touches Windows registers.
#[derive(Debug, Clone)]
pub struct MockBackend {
    pub units: u64,
    pub msr: u64,
    pub mmio: u64,
    pub fail_next_msr_write: bool,
}

impl Default for MockBackend {
    fn default() -> Self {
        let units = 3;
        let old = (1u64 << 15) | (1u64 << 47) | (0x15u64 << 17) | (0x12u64 << 49);
        let encoded = with_power_limits(old, Units::decode(units), 55, 157)
            .expect("valid mock power limits");
        Self { units, msr: encoded, mmio: encoded, fail_next_msr_write: false }
    }
}

impl HardwareBackend for MockBackend {
    fn identity(&mut self) -> Result<Identity, String> {
        Ok(Identity {
            vendor: "GenuineIntel".into(),
            model: "Intel(R) Core(TM) i9-13950HX (SIMULATION)".into(),
            mmio_verified: true,
            simulated: true,
        })
    }
    fn read_units(&mut self) -> Result<u64, String> { Ok(self.units) }
    fn read_msr_limits(&mut self) -> Result<u64, String> { Ok(self.msr) }
    fn read_mmio_limits(&mut self) -> Result<u64, String> { Ok(self.mmio) }
    fn write_msr_limits(&mut self, value: u64) -> Result<(), String> {
        if self.fail_next_msr_write {
            self.fail_next_msr_write = false;
            return Err("simulated MSR write failure".into());
        }
        self.msr = value;
        Ok(())
    }
    fn write_mmio_limits(&mut self, value: u64) -> Result<(), String> {
        self.mmio = value;
        Ok(())
    }
}
