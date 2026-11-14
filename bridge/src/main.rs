//! rapl-bridge: the only user-mode process that talks to the RaplPkg driver.
//!
//! Fixed register whitelist, strict hexadecimal arguments, a least-privilege device
//! handle, the shared write policy and an audit record that is persisted before
//! every register write.
#![cfg_attr(not(windows), allow(dead_code))]

mod abi;
#[cfg(windows)]
mod device;

use abi::{
    Info, RegisterId, FLAG_CPU_SUPPORTED, FLAG_MMIO_READABLE, FLAG_MMIO_VERIFIED, FLAG_READ_ONLY,
    VENDOR_INTEL,
};
use rapl_core::{
    audit::{
        audited_write, hex, record_refusal, sanitize_label, sanitize_text, AuditPhase,
        WriteContext, MAX_DETAIL_CHARS,
    },
    parse_hex_u64,
    policy::check_register_write,
    protocol::{format_info, Identity},
    rapl::Units,
};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const SOURCE: &str = "rapl-bridge";
const USAGE: &str = "usage: rapl-bridge info | msr-read 0x606|0x610 | mmio-read 0x59a0 | \
                     msr-write 0x610 0xVALUE | mmio-write 0x59a0 0xVALUE";

/// Driver operations; implemented by the Windows device and by test fakes.
trait Driver {
    fn info(&mut self) -> Result<Info, String>;
    fn read(&mut self, register: RegisterId) -> Result<u64, String>;
    /// Writes `value` only while the register still holds `expected`; returns the
    /// driver's readback.
    fn write(&mut self, register: RegisterId, expected: u64, value: u64) -> Result<u64, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Info,
    Read(RegisterId),
    Write(RegisterId, u64),
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    let number = |text: &String| parse_hex_u64(text).map_err(|e| format!("{e}; {USAGE}"));
    let whitelist =
        || Err("Register is not in the fixed whitelist (0x606, 0x610, 0x59a0)".to_string());
    match args {
        [command] if command == "info" => Ok(Command::Info),
        [command, register] if command == "msr-read" => match number(register)? {
            0x606 => Ok(Command::Read(RegisterId::Units)),
            0x610 => Ok(Command::Read(RegisterId::PackageLimit)),
            _ => whitelist(),
        },
        [command, register] if command == "mmio-read" => match number(register)? {
            0x59a0 => Ok(Command::Read(RegisterId::MmioPackageLimit)),
            _ => whitelist(),
        },
        [command, register, value] if command == "msr-write" => match number(register)? {
            0x610 => Ok(Command::Write(RegisterId::PackageLimit, number(value)?)),
            _ => whitelist(),
        },
        [command, register, value] if command == "mmio-write" => match number(register)? {
            0x59a0 => Ok(Command::Write(RegisterId::MmioPackageLimit, number(value)?)),
            _ => whitelist(),
        },
        _ => Err(USAGE.into()),
    }
}

/// Audit settings passed by the CLI or the watchdog through the environment.
#[derive(Debug, Default)]
struct AuditEnv {
    log: Option<PathBuf>,
    reason: Option<String>,
    phase: Option<AuditPhase>,
}

impl AuditEnv {
    fn from_env(get: impl Fn(&str) -> Option<OsString>) -> Result<Self, String> {
        let log = get("RAPL_AUDIT_LOG")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let reason = get("RAPL_AUDIT_REASON")
            .map(|value| sanitize_label(&value.to_string_lossy()))
            .filter(|value| !value.is_empty());
        let phase = match get("RAPL_AUDIT_PHASE") {
            None => None,
            Some(value) => Some(
                AuditPhase::parse(&value.to_string_lossy())
                    .ok_or("RAPL_AUDIT_PHASE must be apply or rollback")?,
            ),
        };
        Ok(Self { log, reason, phase })
    }
}

fn identity(info: &Info, model: &str) -> Identity {
    let vendor = if info.vendor == VENDOR_INTEL {
        "GenuineIntel"
    } else {
        "unknown"
    };
    Identity {
        vendor: vendor.into(),
        model: model.into(),
        cpu_family: info.family,
        cpu_model: info.model,
        cpu_stepping: info.stepping,
        packages: info.packages,
        cpu_supported: info.flags & FLAG_CPU_SUPPORTED != 0,
        mmio_readable: info.flags & FLAG_MMIO_READABLE != 0,
        mmio_verified: info.flags & FLAG_MMIO_VERIFIED != 0,
        live_writes_enabled: info.flags & FLAG_READ_ONLY == 0,
        mchbar_base: (info.mchbar_base != 0).then_some(info.mchbar_base),
        simulated: false,
    }
}

fn run(
    command: Command,
    driver: &mut dyn Driver,
    audit: &AuditEnv,
    model: impl FnOnce() -> String,
) -> Result<String, String> {
    match command {
        Command::Info => Ok(format_info(&identity(&driver.info()?, &model()))),
        Command::Read(register) => driver.read(register).map(hex),
        Command::Write(register, value) => write(driver, register, value, audit),
    }
}

/// Gate checks, policy check and an audited compare-and-write.
fn write(
    driver: &mut dyn Driver,
    register: RegisterId,
    requested: u64,
    audit: &AuditEnv,
) -> Result<String, String> {
    let log = audit
        .log
        .as_deref()
        .ok_or("Register writes require RAPL_AUDIT_LOG (append-only JSONL audit log)")?;
    let (command, _) = register
        .write_operation()
        .ok_or("The register is read-only")?;
    let info = driver.info()?;
    if info.flags & FLAG_CPU_SUPPORTED == 0 {
        return Err("The driver reports this CPU as unsupported".into());
    }
    if info.flags & FLAG_READ_ONLY != 0 {
        return Err("The driver is read-only (live writes are disabled)".into());
    }
    let mmio_gates = FLAG_MMIO_READABLE | FLAG_MMIO_VERIFIED;
    if register == RegisterId::MmioPackageLimit && info.flags & mmio_gates != mmio_gates {
        return Err("MMIO writes require a readable, validated MCHBAR+0x59A0 layout".into());
    }
    let units = Units::decode(driver.read(RegisterId::Units)?);
    let current = driver.read(register)?;
    let context = WriteContext {
        log,
        source: SOURCE,
        command,
        register: register.name(),
        units,
        reason: audit.reason.as_deref(),
        phase: audit.phase,
        simulated: false,
    };
    if current != requested {
        if let Err(violation) = check_register_write(current, requested, units) {
            let refusal = format!("Refusing write to {}: {violation}", register.name());
            record_refusal(&context, current, requested, &refusal)
                .map_err(|e| format!("{refusal}; {e}"))?;
            return Err(refusal);
        }
    }
    audited_write(&context, current, requested, || {
        driver.write(register, current, requested)
    })?;
    Ok("OK".into())
}

fn arguments() -> Result<Vec<String>, String> {
    std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "Arguments must be valid Unicode".to_string())
        })
        .collect()
}

#[cfg(windows)]
fn execute() -> Result<String, String> {
    let command = parse_command(&arguments()?)?;
    let audit = AuditEnv::from_env(|key| std::env::var_os(key))?;
    let mut device = device::Device::open(matches!(command, Command::Write(..)))?;
    run(command, &mut device, &audit, device::processor_name)
}

#[cfg(not(windows))]
fn execute() -> Result<String, String> {
    parse_command(&arguments()?)?;
    Err("rapl-bridge is Windows-only; use the CLI with --mock on this host".into())
}

fn main() -> ExitCode {
    match execute() {
        Ok(text) => match writeln!(io::stdout().lock(), "{}", text.trim_end()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        },
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "{}",
                sanitize_text(&error, MAX_DETAIL_CHARS)
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::describe_win32_error;
    use rapl_core::audit::{read_log, AuditRow};
    use rapl_core::protocol::parse_info;
    use rapl_core::rapl::{with_power_limits, LOCK_BIT};

    const UNITS: u64 = 0x000a_0e03;

    struct FakeDriver {
        info: Info,
        msr: u64,
        mmio: u64,
        writes: usize,
        /// Simulates firmware changing the register between read and write.
        race: bool,
    }

    impl FakeDriver {
        fn new() -> Self {
            let base = (0x6eu64 << 17) | (1 << 16) | (0x21u64 << 49) | (1 << 48);
            let value = with_power_limits(base, Units::decode(UNITS), 55, 157).unwrap();
            Self {
                info: Info {
                    size: 40,
                    version: abi::ABI_VERSION,
                    vendor: VENDOR_INTEL,
                    family: 6,
                    model: 0xb7,
                    stepping: 1,
                    flags: FLAG_CPU_SUPPORTED | FLAG_MMIO_READABLE | FLAG_MMIO_VERIFIED,
                    packages: 1,
                    mchbar_base: 0xfed1_0000,
                },
                msr: value,
                mmio: value,
                writes: 0,
                race: false,
            }
        }

        fn target(&self, pl1: u16, pl2: u16) -> u64 {
            with_power_limits(self.msr, Units::decode(UNITS), pl1, pl2).unwrap()
        }
    }

    impl Driver for FakeDriver {
        fn info(&mut self) -> Result<Info, String> {
            Ok(self.info)
        }

        fn read(&mut self, register: RegisterId) -> Result<u64, String> {
            Ok(match register {
                RegisterId::Units => UNITS,
                RegisterId::PackageLimit => self.msr,
                RegisterId::MmioPackageLimit => self.mmio,
            })
        }

        fn write(
            &mut self,
            register: RegisterId,
            expected: u64,
            value: u64,
        ) -> Result<u64, String> {
            let race = self.race;
            let slot = match register {
                RegisterId::Units => return Err("read-only".into()),
                RegisterId::PackageLimit => &mut self.msr,
                RegisterId::MmioPackageLimit => &mut self.mmio,
            };
            if race {
                *slot ^= 1 << 3;
            }
            if *slot != expected {
                return Err(describe_win32_error(1237));
            }
            *slot = value;
            self.writes += 1;
            Ok(value)
        }
    }

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    fn unknown() -> String {
        "unknown".into()
    }

    fn audit_env(name: &str) -> AuditEnv {
        let log =
            std::env::temp_dir().join(format!("rapl_bridge_{}_{name}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&log);
        AuditEnv {
            log: Some(log),
            reason: Some("unit test".into()),
            phase: Some(AuditPhase::Apply),
        }
    }

    fn rows(audit: &AuditEnv) -> Vec<AuditRow> {
        let log = audit.log.as_ref().unwrap();
        match std::fs::read(log) {
            Ok(bytes) => read_log(bytes.as_slice(), None).unwrap().rows,
            Err(_) => Vec::new(),
        }
    }

    fn cleanup(audit: &AuditEnv) {
        let _ = std::fs::remove_file(audit.log.as_ref().unwrap());
    }

    #[test]
    fn parses_only_whitelisted_commands() {
        let cases = [
            ("info", Command::Info),
            ("msr-read 0x606", Command::Read(RegisterId::Units)),
            ("msr-read 0x610", Command::Read(RegisterId::PackageLimit)),
            (
                "mmio-read 0x59A0",
                Command::Read(RegisterId::MmioPackageLimit),
            ),
            (
                "msr-write 0x610 0x1",
                Command::Write(RegisterId::PackageLimit, 1),
            ),
            (
                "mmio-write 0x59a0 0xff",
                Command::Write(RegisterId::MmioPackageLimit, 0xff),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_command(&args(text)), Ok(expected), "{text}");
        }
        for bad in [
            "",
            "info extra",
            "msr-read 0x611",
            "msr-read 610",
            "msr-write 0x606 0x1",
            "mmio-read 0x610",
            "mmio-write 0x59a0",
            "msr-write 0x610 0x1 0x2",
            "msr-write 0x610 -0x1",
            "msr-write 0x610 0x10000000000000000",
        ] {
            assert!(parse_command(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn info_reports_protocol_2_gates() {
        let mut driver = FakeDriver::new();
        let model = || "Intel(R) Core(TM) i9-13950HX".to_string();
        let text = run(Command::Info, &mut driver, &AuditEnv::default(), model).unwrap();
        let identity = parse_info(&text).unwrap();
        assert!(identity.cpu_supported && identity.mmio_readable && identity.mmio_verified);
        assert!(identity.live_writes_enabled);
        assert_eq!(identity.mchbar_base, Some(0xfed1_0000));
        driver.info.flags = FLAG_READ_ONLY;
        driver.info.mchbar_base = 0;
        let text = run(Command::Info, &mut driver, &AuditEnv::default(), unknown).unwrap();
        let identity = parse_info(&text).unwrap();
        assert!(!identity.cpu_supported && !identity.mmio_readable);
        assert!(!identity.live_writes_enabled);
        assert_eq!(identity.mchbar_base, None);
    }

    #[test]
    fn reads_print_fixed_width_hex() {
        let mut driver = FakeDriver::new();
        let text = run(
            Command::Read(RegisterId::Units),
            &mut driver,
            &AuditEnv::default(),
            unknown,
        );
        assert_eq!(text, Ok("0x00000000000a0e03".into()));
    }

    #[test]
    fn writes_require_an_audit_log() {
        let mut driver = FakeDriver::new();
        let target = driver.target(45, 100);
        let command = Command::Write(RegisterId::PackageLimit, target);
        let error = run(command, &mut driver, &AuditEnv::default(), unknown).unwrap_err();
        assert!(error.contains("RAPL_AUDIT_LOG"), "{error}");
        assert_eq!(driver.writes, 0);
    }

    #[test]
    fn audited_write_records_watt_columns() {
        let audit = audit_env("write");
        let mut driver = FakeDriver::new();
        let target = driver.target(45, 100);
        let command = Command::Write(RegisterId::PackageLimit, target);
        assert_eq!(run(command, &mut driver, &audit, unknown), Ok("OK".into()));
        assert_eq!(driver.msr, target);
        assert_eq!(run(command, &mut driver, &audit, unknown), Ok("OK".into()));
        assert_eq!(driver.writes, 1, "an unchanged value is not written again");
        let rows = rows(&audit);
        let outcomes: Vec<_> = rows.iter().map(|r| r.outcome.clone().unwrap()).collect();
        assert_eq!(outcomes, ["pending", "changed", "no_change"]);
        let changed = &rows[1];
        assert_eq!(changed.source.as_deref(), Some(SOURCE));
        assert_eq!(changed.register.as_deref(), Some("MSR_0x610"));
        assert_eq!(changed.phase.as_deref(), Some("apply"));
        assert_eq!(changed.reason.as_deref(), Some("unit test"));
        assert_eq!(
            (changed.pl1_before_w, changed.pl2_before_w),
            (Some(55.0), Some(157.0))
        );
        assert_eq!(
            (changed.pl1_set_w, changed.pl2_set_w),
            (Some(45.0), Some(100.0))
        );
        assert_eq!(changed.pl1_readback_w, Some(45.0));
        assert!(!changed.simulated);
        cleanup(&audit);
    }

    #[test]
    fn refuses_policy_violations_and_audits_them() {
        let audit = audit_env("refuse");
        let mut driver = FakeDriver::new();
        let target = driver.target(45, 100);
        let too_high = (target & !(0x7fff << 32)) | ((158 * 8) << 32);
        let protected = target ^ (1 << 17);
        let disabled = target & !(1 << 15);
        for value in [too_high, protected, disabled] {
            let command = Command::Write(RegisterId::PackageLimit, value);
            let error = run(command, &mut driver, &audit, unknown).unwrap_err();
            assert!(error.starts_with("Refusing write to MSR_0x610"), "{error}");
        }
        driver.mmio |= LOCK_BIT;
        let command = Command::Write(RegisterId::MmioPackageLimit, target | LOCK_BIT);
        let error = run(command, &mut driver, &audit, unknown).unwrap_err();
        assert!(error.contains("locked"), "{error}");
        assert_eq!(driver.writes, 0);
        let rows = rows(&audit);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.outcome.as_deref() == Some("refused")));
        assert!(rows[0].detail.as_deref().unwrap().contains("PL1 <= PL2"));
        assert_eq!(rows[0].pl2_set_w, Some(158.0));
        cleanup(&audit);
    }

    #[test]
    fn honours_driver_gates_before_auditing() {
        let audit = audit_env("gates");
        let mut driver = FakeDriver::new();
        let target = driver.target(45, 100);
        driver.info.flags &= !FLAG_MMIO_VERIFIED;
        let mmio = Command::Write(RegisterId::MmioPackageLimit, target);
        assert!(run(mmio, &mut driver, &audit, unknown).is_err());
        driver.info.flags |= FLAG_READ_ONLY;
        let msr = Command::Write(RegisterId::PackageLimit, target);
        assert!(run(msr, &mut driver, &audit, unknown).is_err());
        driver.info.flags = FLAG_MMIO_READABLE | FLAG_MMIO_VERIFIED;
        assert!(run(msr, &mut driver, &audit, unknown).is_err());
        assert_eq!(driver.writes, 0);
        assert!(rows(&audit).is_empty());
        cleanup(&audit);
    }

    #[test]
    fn concurrent_change_is_reported_and_audited() {
        let audit = audit_env("race");
        let mut driver = FakeDriver::new();
        driver.race = true;
        let target = driver.target(45, 100);
        let command = Command::Write(RegisterId::PackageLimit, target);
        let error = run(command, &mut driver, &audit, unknown).unwrap_err();
        assert!(error.contains("re-run plan"), "{error}");
        let outcomes: Vec<_> = rows(&audit)
            .iter()
            .map(|r| r.outcome.clone().unwrap())
            .collect();
        assert_eq!(outcomes, ["pending", "driver_error"]);
        cleanup(&audit);
    }

    #[test]
    fn reads_audit_settings_strictly() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        assert!(AuditEnv::from_env(env(&[("RAPL_AUDIT_PHASE", "bogus")])).is_err());
        let parsed = AuditEnv::from_env(env(&[
            ("RAPL_AUDIT_LOG", ""),
            ("RAPL_AUDIT_REASON", "line\nbreak"),
            ("RAPL_AUDIT_PHASE", "rollback"),
        ]))
        .unwrap();
        assert_eq!(parsed.log, None);
        assert_eq!(parsed.reason.as_deref(), Some("line break"));
        assert_eq!(parsed.phase, Some(AuditPhase::Rollback));
    }
}
