//! Hardware access facade. No kernel drivers or unsigned binaries are bundled.
//! Real hardware I/O is delegated to a separately audited, local Windows bridge.
use crate::rapl::{with_power_limits, Units};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize)]
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
    timeout: Duration,
    audit_log: Option<PathBuf>,
}

impl ExternalBridge {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        Self::with_timeout(path, Duration::from_secs(10))
    }

    pub fn with_timeout(path: PathBuf, timeout: Duration) -> Result<Self, String> {
        if !cfg!(windows) {
            return Err("Live bridge requires Windows; use --mock here".into());
        }
        if timeout.is_zero() {
            return Err("Bridge timeout must be positive".into());
        }
        let full = path
            .canonicalize()
            .map_err(|e| format!("Bridge executable cannot be resolved: {e}"))?;
        if !full.is_file()
            || full
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| !e.eq_ignore_ascii_case("exe"))
                .unwrap_or(true)
        {
            return Err("Bridge must point to an existing .exe".into());
        }
        Ok(Self {
            path: full,
            timeout,
            audit_log: None,
        })
    }

    pub fn with_audit_log(mut self, path: Option<PathBuf>) -> Self {
        self.audit_log = path;
        self
    }

    fn call(&self, args: &[&str]) -> Result<String, String> {
        run_process(&self.path, args, self.timeout, self.audit_log.as_deref())
    }

    fn read_hex(&self, args: &[&str]) -> Result<u64, String> {
        let output = self.call(args)?;
        let value = output
            .strip_prefix("0x")
            .or_else(|| output.strip_prefix("0X"))
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
        parse_identity(&result)
    }

    fn read_units(&mut self) -> Result<u64, String> {
        self.read_hex(&["msr-read", "0x606"])
    }
    fn read_msr_limits(&mut self) -> Result<u64, String> {
        self.read_hex(&["msr-read", "0x610"])
    }
    fn read_mmio_limits(&mut self) -> Result<u64, String> {
        self.read_hex(&["mmio-read", "0x59a0"])
    }
    fn write_msr_limits(&mut self, value: u64) -> Result<(), String> {
        self.write_hex("msr-write", "0x610", value)
    }
    fn write_mmio_limits(&mut self, value: u64) -> Result<(), String> {
        self.write_hex("mmio-write", "0x59a0", value)
    }
}

// Cap each pipe while reading, not after unbounded Command::output allocation.
fn run_process(
    path: &std::path::Path,
    args: &[&str],
    timeout: Duration,
    audit_log: Option<&std::path::Path>,
) -> Result<String, String> {
    let mut command = Command::new(path);
    command.args(args);
    if let Some(log) = audit_log {
        command.env("RAPL_AUDIT_LOG", log);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot execute hardware bridge: {e}"))?;
    let (tx, rx) = mpsc::channel();
    fn reader<R: Read + Send + 'static>(
        pipe: R,
        tx: mpsc::Sender<(bool, Result<Vec<u8>, String>)>,
        stdout: bool,
    ) {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe
                .take(4097)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Cannot read bridge pipe: {e}"))
                .and_then(|_| {
                    if bytes.len() > 4096 {
                        Err("Bridge output exceeds 4096 bytes".into())
                    } else {
                        Ok(bytes)
                    }
                });
            let _ = tx.send((stdout, result));
        });
    }
    reader(child.stdout.take().unwrap(), tx.clone(), true);
    reader(child.stderr.take().unwrap(), tx, false);
    let start = Instant::now();
    let mut stdout = None;
    let mut stderr = None;
    let result = (|| loop {
        while let Ok((is_stdout, value)) = rx.try_recv() {
            if is_stdout {
                stdout = Some(value?);
            } else {
                stderr = Some(value?);
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if let (Some(out), Some(err)) = (&stdout, &stderr) {
                if !status.success() {
                    return Err(format!(
                        "Bridge returned {status}: {}",
                        String::from_utf8_lossy(err).trim()
                    ));
                }
                return String::from_utf8(out.clone())
                    .map(|s| s.trim().to_owned())
                    .map_err(|_| "Bridge response is not UTF-8".into());
            }
        }
        if start.elapsed() >= timeout {
            return Err("Bridge timed out; write outcome may be unknown".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

fn parse_identity(result: &str) -> Result<Identity, String> {
    let mut fields = std::collections::BTreeMap::new();
    for line in result.lines() {
        let (key, value) = line.split_once('=').ok_or("Malformed bridge info line")?;
        if fields.insert(key.trim(), value.trim()).is_some() {
            return Err(format!("Duplicate bridge info field: {key}"));
        }
    }
    if fields.get("protocol") != Some(&"1") {
        return Err("Unsupported bridge protocol (expected protocol=1)".into());
    }
    let field = |key| {
        fields
            .get(key)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Missing bridge info field: {key}"))
    };
    let verified = match fields.get("mmio_59a0_64bit_verified") {
        Some(&"true") => true,
        Some(&"false") => false,
        _ => return Err("Invalid/missing MMIO verification flag".into()),
    };
    Ok(Identity {
        vendor: field("vendor")?,
        model: field("model")?,
        mmio_verified: verified,
        simulated: false,
    })
}

/// In-memory test backend. NEVER touches Windows registers.
#[derive(Debug, Clone, serde::Serialize)]
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
        let encoded =
            with_power_limits(old, Units::decode(units), 55, 157).expect("valid mock power limits");
        Self {
            units,
            msr: encoded,
            mmio: encoded,
            fail_next_msr_write: false,
        }
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
    fn read_units(&mut self) -> Result<u64, String> {
        Ok(self.units)
    }
    fn read_msr_limits(&mut self) -> Result<u64, String> {
        Ok(self.msr)
    }
    fn read_mmio_limits(&mut self) -> Result<u64, String> {
        Ok(self.mmio)
    }
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
