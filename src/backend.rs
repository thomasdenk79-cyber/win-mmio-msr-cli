//! Hardware access facade. No kernel drivers or unsigned binaries are bundled.
//! Real hardware I/O is delegated to a separately audited, local Windows bridge.
use crate::audit::{
    audited_write, hex, sanitize_label, sanitize_text, AuditPhase, WriteContext, MAX_DETAIL_CHARS,
    REGISTER_MMIO_PKG_LIMIT, REGISTER_MSR_PKG_LIMIT,
};
use crate::rapl::{with_power_limits, Units};
use crate::{SUPPORTED_CPU_FAMILY, SUPPORTED_CPU_MODEL};
use rapl_core::parse_hex_u64;
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Text protocol version of `rapl-bridge info`.
pub const BRIDGE_PROTOCOL_VERSION: &str = "2";
/// Largest accepted stdout or stderr of one bridge call.
pub const MAX_BRIDGE_OUTPUT: usize = 4096;

/// CPU identity and driver capability gates reported by the backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub vendor: String,
    pub model: String,
    pub cpu_family: u32,
    pub cpu_model: u32,
    pub cpu_stepping: u32,
    pub packages: u32,
    /// The driver's own CPU gate (vendor, family/model and a single package).
    pub cpu_supported: bool,
    /// The driver can read MCHBAR+0x59A0.
    pub mmio_readable: bool,
    /// The MCHBAR+0x59A0 layout was validated for this platform.
    pub mmio_verified: bool,
    /// The driver accepts register writes.
    pub live_writes_enabled: bool,
    #[serde(serialize_with = "serialize_optional_hex")]
    pub mchbar_base: Option<u64>,
    pub simulated: bool,
}

fn serialize_optional_hex<S: serde::Serializer>(
    value: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_str(&hex(*value)),
        None => serializer.serialize_none(),
    }
}

pub trait HardwareBackend {
    fn identity(&mut self) -> Result<Identity, String>;
    fn read_units(&mut self) -> Result<u64, String>;
    fn read_msr_limits(&mut self) -> Result<u64, String>;
    fn read_mmio_limits(&mut self) -> Result<u64, String>;
    fn write_msr_limits(&mut self, value: u64) -> Result<(), String>;
    fn write_mmio_limits(&mut self, value: u64) -> Result<(), String>;
    /// Labels subsequent writes in the audit log (apply or rollback).
    fn set_audit_phase(&mut self, _phase: AuditPhase) {}
}

pub struct ExternalBridge {
    path: PathBuf,
    timeout: Duration,
    audit_log: Option<PathBuf>,
    audit_reason: Option<String>,
    audit_phase: Option<AuditPhase>,
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
        let is_exe = full
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        if !full.is_file() || !is_exe {
            return Err("Bridge must point to an existing .exe".into());
        }
        Ok(Self {
            path: full,
            timeout,
            audit_log: None,
            audit_reason: None,
            audit_phase: None,
        })
    }

    pub fn with_audit_log(mut self, path: Option<PathBuf>) -> Self {
        self.audit_log = path;
        self
    }

    pub fn with_audit_reason(mut self, reason: Option<String>) -> Self {
        self.audit_reason = reason
            .map(|reason| sanitize_label(&reason))
            .filter(|reason| !reason.is_empty());
        self
    }

    /// Audit settings for the bridge. Unset values are removed from the child
    /// environment so inherited variables cannot redirect or relabel records.
    fn environment(&self) -> [(&'static str, Option<OsString>); 3] {
        [
            ("RAPL_AUDIT_LOG", self.audit_log.clone().map(OsString::from)),
            (
                "RAPL_AUDIT_REASON",
                self.audit_reason.clone().map(OsString::from),
            ),
            (
                "RAPL_AUDIT_PHASE",
                self.audit_phase.map(|phase| OsString::from(phase.as_str())),
            ),
        ]
    }

    fn call(&self, args: &[&str]) -> Result<String, String> {
        run_process(&self.path, args, self.timeout, &self.environment())
    }

    fn read_hex(&self, args: &[&str]) -> Result<u64, String> {
        let output = self.call(args)?;
        parse_hex_u64(&output).map_err(|e| format!("Bridge read output: {e}"))
    }

    fn write_hex(&self, kind: &str, address: &str, value: u64) -> Result<(), String> {
        if self.audit_log.is_none() {
            return Err("Register writes require an audit log (--audit-log)".into());
        }
        let result = self.call(&[kind, address, &hex(value)])?;
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
    fn set_audit_phase(&mut self, phase: AuditPhase) {
        self.audit_phase = Some(phase);
    }
}

type PipeResult = (bool, Result<Vec<u8>, String>);

fn spawn_reader(pipe: impl Read + Send + 'static, tx: mpsc::Sender<PipeResult>, stdout: bool) {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe
            .take(MAX_BRIDGE_OUTPUT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read bridge pipe: {e}"))
            .and_then(|_| {
                if bytes.len() > MAX_BRIDGE_OUTPUT {
                    Err(format!("Bridge output exceeds {MAX_BRIDGE_OUTPUT} bytes"))
                } else {
                    Ok(bytes)
                }
            });
        let _ = tx.send((stdout, result));
    });
}

/// Runs one bridge call with bounded pipes and a hard timeout. Environment entries
/// with `None` are removed from the child.
fn run_process(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    environment: &[(&str, Option<OsString>)],
) -> Result<String, String> {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in environment {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot execute hardware bridge: {e}"))?;
    let (Some(out_pipe), Some(err_pipe)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Cannot capture bridge output".into());
    };
    let (tx, rx) = mpsc::channel();
    spawn_reader(out_pipe, tx.clone(), true);
    spawn_reader(err_pipe, tx, false);
    let deadline = Instant::now() + timeout;
    let mut stdout = None;
    let mut stderr = None;
    let result = loop {
        let mut pipe_error = None;
        while let Ok((is_stdout, value)) = rx.try_recv() {
            match value {
                Ok(bytes) if is_stdout => stdout = Some(bytes),
                Ok(bytes) => stderr = Some(bytes),
                Err(error) => pipe_error = Some(error),
            }
        }
        if let Some(error) = pipe_error {
            break Err(error);
        }
        if let (Some(out), Some(err)) = (&stdout, &stderr) {
            match child.try_wait() {
                Err(e) => break Err(format!("Cannot wait for bridge: {e}")),
                Ok(Some(status)) if !status.success() => {
                    let message = String::from_utf8_lossy(err);
                    break Err(format!(
                        "Bridge returned {status}: {}",
                        sanitize_text(message.trim(), MAX_DETAIL_CHARS)
                    ));
                }
                Ok(Some(_)) => {
                    break String::from_utf8(out.clone())
                        .map(|text| text.trim().to_owned())
                        .map_err(|_| "Bridge response is not UTF-8".into());
                }
                Ok(None) => {}
            }
        }
        if Instant::now() >= deadline {
            break Err("Bridge timed out; write outcome may be unknown".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

/// Parses `rapl-bridge info` (protocol 2). Unknown keys are ignored for forward
/// compatibility; missing, duplicate or inconsistent fields are rejected.
fn parse_identity(text: &str) -> Result<Identity, String> {
    let mut fields = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("Malformed bridge info line: {line}"))?;
        if fields.insert(key.trim(), value.trim()).is_some() {
            return Err(format!("Duplicate bridge info field: {}", key.trim()));
        }
    }
    let get = |key: &str| -> Result<&str, String> {
        fields
            .get(key)
            .copied()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("Missing bridge info field: {key}"))
    };
    let protocol = get("protocol")?;
    if protocol != BRIDGE_PROTOCOL_VERSION {
        return Err(format!(
            "Unsupported bridge protocol {protocol} (expected protocol={BRIDGE_PROTOCOL_VERSION})"
        ));
    }
    let flag = |key: &str| -> Result<bool, String> {
        match get(key)? {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("Invalid boolean for {key}: {other}")),
        }
    };
    let hex32 = |key: &str| -> Result<u32, String> {
        let value = parse_hex_u64(get(key)?).map_err(|e| format!("{key}: {e}"))?;
        u32::try_from(value).map_err(|_| format!("{key} is out of range"))
    };
    let packages = get("packages")?
        .parse::<u32>()
        .map_err(|_| "Invalid bridge info field: packages".to_string())?;
    let mchbar_base = match get("mchbar_base")? {
        "unavailable" => None,
        value => Some(parse_hex_u64(value).map_err(|e| format!("mchbar_base: {e}"))?),
    };
    let identity = Identity {
        vendor: sanitize_label(get("vendor")?),
        model: sanitize_label(get("model")?),
        cpu_family: hex32("cpu_family")?,
        cpu_model: hex32("cpu_model")?,
        cpu_stepping: hex32("cpu_stepping")?,
        packages,
        cpu_supported: flag("cpu_supported")?,
        mmio_readable: flag("mmio_readable")?,
        mmio_verified: flag("mmio_59a0_64bit_verified")?,
        live_writes_enabled: flag("live_writes_enabled")?,
        mchbar_base,
        simulated: false,
    };
    if identity.mmio_verified && !identity.mmio_readable {
        return Err("Inconsistent bridge info: MMIO layout verified but MMIO not readable".into());
    }
    if identity.mmio_readable && identity.mchbar_base.is_none() {
        return Err("Inconsistent bridge info: MMIO readable without an MCHBAR base".into());
    }
    Ok(identity)
}

/// Raw MSR 0x606 value of the simulation: 1/8 W power unit, 2^-10 s time unit.
pub const MOCK_UNITS: u64 = 0x000a_0e03;
/// Simulated MCHBAR base, consistent with the MicroPL address 0xFED159A0.
pub const MOCK_MCHBAR_BASE: u64 = 0xfed1_0000;

/// In-memory test backend. NEVER touches hardware.
#[derive(Debug, Clone)]
pub struct MockBackend {
    pub units: u64,
    pub msr: u64,
    pub mmio: u64,
    pub mmio_readable: bool,
    pub mmio_verified: bool,
    pub live_writes_enabled: bool,
    pub fail_next_msr_write: bool,
    audit_log: Option<PathBuf>,
    audit_reason: Option<String>,
    audit_phase: Option<AuditPhase>,
}

impl Default for MockBackend {
    fn default() -> Self {
        // Both limits enabled and clamped; tau codes 0x6e (28 s) and 0x21 (~2.4 ms).
        let old = (1u64 << 15)
            | (1u64 << 16)
            | (0x6eu64 << 17)
            | (1u64 << 47)
            | (1u64 << 48)
            | (0x21u64 << 49);
        let encoded = with_power_limits(old, Units::decode(MOCK_UNITS), 55, 157)
            .expect("valid mock power limits");
        Self {
            units: MOCK_UNITS,
            msr: encoded,
            mmio: encoded,
            mmio_readable: true,
            mmio_verified: true,
            live_writes_enabled: true,
            fail_next_msr_write: false,
            audit_log: None,
            audit_reason: None,
            audit_phase: None,
        }
    }
}

impl MockBackend {
    /// Records simulated writes (`simulated: true`) in a JSONL audit log.
    pub fn with_audit_log(mut self, log: Option<PathBuf>, reason: Option<String>) -> Self {
        self.audit_log = log;
        self.audit_reason = reason.map(|reason| sanitize_label(&reason));
        self
    }

    fn write(&mut self, mmio: bool, value: u64) -> Result<(), String> {
        if !self.live_writes_enabled {
            return Err("simulated driver is read-only".into());
        }
        if mmio && !(self.mmio_readable && self.mmio_verified) {
            return Err("simulated MMIO writes are disabled".into());
        }
        let fail = !mmio && std::mem::take(&mut self.fail_next_msr_write);
        let (command, register) = if mmio {
            ("mmio-write", REGISTER_MMIO_PKG_LIMIT)
        } else {
            ("msr-write", REGISTER_MSR_PKG_LIMIT)
        };
        let slot = if mmio { &mut self.mmio } else { &mut self.msr };
        let previous = *slot;
        let mut store = move || -> Result<u64, String> {
            if fail {
                return Err("simulated MSR write failure".into());
            }
            *slot = value;
            Ok(value)
        };
        match &self.audit_log {
            Some(log) => {
                let context = WriteContext {
                    log,
                    source: "mock",
                    command,
                    register,
                    units: Units::decode(self.units),
                    reason: self.audit_reason.as_deref(),
                    phase: self.audit_phase,
                    simulated: true,
                };
                audited_write(&context, previous, value, store).map(|_| ())
            }
            None => store().map(|_| ()),
        }
    }
}

impl HardwareBackend for MockBackend {
    fn identity(&mut self) -> Result<Identity, String> {
        Ok(Identity {
            vendor: "GenuineIntel".into(),
            model: "Intel(R) Core(TM) i9-13950HX (SIMULATION)".into(),
            cpu_family: SUPPORTED_CPU_FAMILY,
            cpu_model: SUPPORTED_CPU_MODEL,
            cpu_stepping: 1,
            packages: 1,
            cpu_supported: true,
            mmio_readable: self.mmio_readable,
            mmio_verified: self.mmio_verified,
            live_writes_enabled: self.live_writes_enabled,
            mchbar_base: self.mmio_readable.then_some(MOCK_MCHBAR_BASE),
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
        if !self.mmio_readable {
            return Err("simulated MMIO read is disabled".into());
        }
        Ok(self.mmio)
    }
    fn write_msr_limits(&mut self, value: u64) -> Result<(), String> {
        self.write(false, value)
    }
    fn write_mmio_limits(&mut self, value: u64) -> Result<(), String> {
        self.write(true, value)
    }
    fn set_audit_phase(&mut self, phase: AuditPhase) {
        self.audit_phase = Some(phase);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "protocol=2\nvendor=GenuineIntel\nmodel=Intel(R) Core(TM) i9-13950HX\n\
        cpu_family=0x6\ncpu_model=0xb7\ncpu_stepping=0x1\npackages=1\ncpu_supported=true\n\
        mmio_readable=true\nmmio_59a0_64bit_verified=false\nlive_writes_enabled=false\n\
        mchbar_base=0x00000000fed10000\n";

    fn replace(key: &str, value: &str) -> String {
        INFO.lines()
            .map(|line| {
                if line.starts_with(&format!("{key}=")) {
                    format!("{key}={value}")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parses_protocol_2_identity() {
        let identity = parse_identity(&format!("{INFO}future_key=ignored\n")).unwrap();
        assert_eq!(identity.cpu_family, 6);
        assert_eq!(identity.cpu_model, 0xb7);
        assert_eq!(identity.packages, 1);
        assert!(identity.cpu_supported && identity.mmio_readable);
        assert!(!identity.mmio_verified && !identity.live_writes_enabled);
        assert_eq!(identity.mchbar_base, Some(0xfed1_0000));
        assert!(!identity.simulated);
    }

    #[test]
    fn rejects_bad_identity() {
        let cases = [
            replace("protocol", "1"),
            replace("cpu_model", "183"),
            replace("cpu_supported", "yes"),
            replace("packages", "-1"),
            replace("mchbar_base", "unavailable"),
            replace("mmio_readable", "false").replace("verified=false", "verified=true"),
            INFO.replace("vendor=GenuineIntel\n", ""),
            format!("{INFO}vendor=AuthenticAMD\n"),
            format!("{INFO}garbage\n"),
        ];
        for case in cases {
            assert!(parse_identity(&case).is_err(), "{case}");
        }
    }

    #[test]
    fn mock_respects_driver_gates() {
        let mut mock = MockBackend {
            live_writes_enabled: false,
            ..MockBackend::default()
        };
        assert!(mock.write_msr_limits(0).is_err());
        mock.live_writes_enabled = true;
        mock.mmio_verified = false;
        assert!(mock.write_mmio_limits(0).is_err());
        mock.mmio_readable = false;
        assert!(mock.read_mmio_limits().is_err());
        assert_eq!(mock.identity().unwrap().mchbar_base, None);
    }

    #[cfg(unix)]
    mod process {
        use super::super::*;

        fn shell(script: &str, timeout_ms: u64) -> Result<String, String> {
            run_process(
                Path::new("/bin/sh"),
                &["-c", script],
                Duration::from_millis(timeout_ms),
                &[("RAPL_AUDIT_PHASE", Some(OsString::from("rollback")))],
            )
        }

        #[test]
        fn returns_trimmed_stdout() {
            assert_eq!(shell("printf ' OK\\n'", 5000), Ok("OK".into()));
        }

        #[test]
        fn passes_audit_environment() {
            assert_eq!(
                shell("printf %s \"$RAPL_AUDIT_PHASE\"", 5000),
                Ok("rollback".into())
            );
        }

        #[test]
        fn reports_failure_with_stderr() {
            let error = shell("echo 'access denied' >&2; exit 3", 5000).unwrap_err();
            assert!(error.contains("access denied"), "{error}");
        }

        #[test]
        fn enforces_timeout() {
            let error = shell("sleep 5", 200).unwrap_err();
            assert!(error.contains("timed out"), "{error}");
        }

        #[test]
        fn caps_output_size() {
            let error = shell("head -c 5000 /dev/zero", 5000).unwrap_err();
            assert!(error.contains("exceeds"), "{error}");
        }
    }
}
