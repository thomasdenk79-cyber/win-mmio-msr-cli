//! High-level, fail-closed orchestration for Intel RAPL package limits.
pub mod backend;
pub use rapl_core::{audit, rapl};

use audit::{
    format_watts, hex, sanitize_label, utc_timestamp, AuditPhase, WattColumns,
    REGISTER_MMIO_PKG_LIMIT, REGISTER_MSR_PKG_LIMIT,
};
use backend::{HardwareBackend, Identity};
use rapl::{decode_limits, with_power_limits, Units};
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

pub const MIN_WATTS: u16 = 5;
/// Intel processor base power of the i9-13950HX.
pub const PROCESSOR_BASE_WATTS: u16 = 55;
/// Intel maximum turbo power of the i9-13950HX.
pub const MAX_TURBO_WATTS: u16 = 157;
pub const SUPPORTED_CPU_FAMILY: u32 = 6;
/// Raptor Lake HX.
pub const SUPPORTED_CPU_MODEL: u32 = 0xb7;
pub const SUPPORTED_PROCESSOR_NAME: &str = "i9-13950HX";
pub const BACKUP_HEADER: &str = "WIN_MMIO_MSR_BACKUP_V2";

/// The single PL1/PL2 policy check shared by the CLI, the planner and the watchdog.
pub fn validate_limits(
    pl1_watts: u16,
    pl2_watts: u16,
    allow_sustained_above_base: bool,
) -> Result<(), String> {
    if pl1_watts < MIN_WATTS || pl2_watts > MAX_TURBO_WATTS || pl2_watts < pl1_watts {
        return Err(format!(
            "Use {MIN_WATTS} <= PL1 <= PL2 <= {MAX_TURBO_WATTS} W (got PL1 {pl1_watts} W, PL2 {pl2_watts} W)"
        ));
    }
    if pl1_watts > PROCESSOR_BASE_WATTS && !allow_sustained_above_base {
        return Err(format!(
            "PL1={pl1_watts} W exceeds Intel's {PROCESSOR_BASE_WATTS} W processor base power. \
             Provide --allow-sustained-above-base to acknowledge this."
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub identity: Identity,
    pub units: Units,
    pub msr: u64,
    /// `None` when the driver does not expose MCHBAR+0x59A0.
    pub mmio: Option<u64>,
}

/// Planned transition of one power-limit register with readable watt columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegisterPlan {
    pub register: &'static str,
    pub path: &'static str,
    pub before: u64,
    pub after: u64,
    pub watts: WattColumns,
}

#[derive(Debug, Clone)]
pub struct Plan {
    before: Snapshot,
    after_msr: u64,
    after_mmio: Option<u64>,
    pl1_watts: u16,
    pl2_watts: u16,
    blockers: Vec<String>,
}

impl Plan {
    pub fn before(&self) -> &Snapshot {
        &self.before
    }

    pub fn watts(&self) -> (u16, u16) {
        (self.pl1_watts, self.pl2_watts)
    }

    /// Reasons why `apply` refuses on this platform; empty when writes are allowed.
    pub fn blockers(&self) -> &[String] {
        &self.blockers
    }

    /// Register transitions in write order: MMIO first, then MSR.
    pub fn changes(&self) -> Vec<RegisterPlan> {
        let units = self.before.units;
        let mut changes = Vec::with_capacity(2);
        if let (Some(before), Some(after)) = (self.before.mmio, self.after_mmio) {
            changes.push(RegisterPlan {
                register: REGISTER_MMIO_PKG_LIMIT,
                path: "MMIO",
                before,
                after,
                watts: WattColumns::new(before, after, units),
            });
        }
        changes.push(RegisterPlan {
            register: REGISTER_MSR_PKG_LIMIT,
            path: "MSR",
            before: self.before.msr,
            after: self.after_msr,
            watts: WattColumns::new(self.before.msr, self.after_msr, units),
        });
        changes
    }

    /// True when both registers already hold the target values.
    pub fn already_applied(&self) -> bool {
        self.after_mmio.is_some() && self.changes().iter().all(|c| c.before == c.after)
    }
}

/// Refuses CPUs outside the validated profile. MMIO gates are checked separately
/// so that read-only status works with MSR access alone.
pub fn validate_identity(id: &Identity) -> Result<(), String> {
    if id.vendor != "GenuineIntel" {
        return Err(format!("Refusing unsupported CPU vendor: {}", id.vendor));
    }
    if id.cpu_family != SUPPORTED_CPU_FAMILY || id.cpu_model != SUPPORTED_CPU_MODEL {
        return Err(format!(
            "Refusing CPU family 0x{:x} model 0x{:x}; only family 0x{SUPPORTED_CPU_FAMILY:x} \
             model 0x{SUPPORTED_CPU_MODEL:x} (Raptor Lake HX) is supported",
            id.cpu_family, id.cpu_model
        ));
    }
    if !id.model.contains(SUPPORTED_PROCESSOR_NAME) {
        return Err(format!(
            "Only the validated Intel {SUPPORTED_PROCESSOR_NAME} profile is supported: {}",
            id.model
        ));
    }
    if !id.cpu_supported {
        return Err("The driver reports this CPU as unsupported".into());
    }
    if id.packages != 1 {
        return Err(format!(
            "Only single-package systems are supported (packages: {})",
            id.packages
        ));
    }
    Ok(())
}

/// Reasons that block writes for this identity; empty when apply is allowed.
pub fn apply_blockers(id: &Identity) -> Vec<String> {
    let mut blockers = Vec::new();
    if !id.mmio_readable {
        blockers.push(
            "MCHBAR+0x59A0 is not readable through the driver, so MSR and MMIO cannot be \
             kept in sync"
                .into(),
        );
    }
    if !id.mmio_verified {
        blockers
            .push("The MCHBAR+0x59A0 register layout is not validated for this platform".into());
    }
    if !id.live_writes_enabled {
        blockers.push("The driver is read-only (live writes are disabled)".into());
    }
    blockers
}

pub fn read_snapshot(backend: &mut dyn HardwareBackend) -> Result<Snapshot, String> {
    let identity = backend.identity()?;
    validate_identity(&identity)?;
    let raw_units = backend.read_units()?;
    if raw_units == u64::MAX {
        return Err("Implausible RAPL units (all bits set)".into());
    }
    let msr = backend.read_msr_limits()?;
    // The bridge returns one coherent 64-bit register; it never guesses addresses.
    let mmio = if identity.mmio_readable {
        Some(backend.read_mmio_limits()?)
    } else {
        None
    };
    if msr == u64::MAX || mmio == Some(u64::MAX) {
        return Err("Implausible register value (all bits set)".into());
    }
    Ok(Snapshot {
        identity,
        units: Units::decode(raw_units),
        msr,
        mmio,
    })
}

pub fn build_plan(
    backend: &mut dyn HardwareBackend,
    pl1_watts: u16,
    pl2_watts: u16,
    allow_sustained_above_base: bool,
) -> Result<Plan, String> {
    validate_limits(pl1_watts, pl2_watts, allow_sustained_above_base)?;
    let before = read_snapshot(backend)?;
    let after_msr = with_power_limits(before.msr, before.units, pl1_watts, pl2_watts)
        .map_err(|e| format!("MSR 0x610: {e}"))?;
    let after_mmio = before
        .mmio
        .map(|raw| {
            with_power_limits(raw, before.units, pl1_watts, pl2_watts)
                .map_err(|e| format!("MMIO MCHBAR+0x59A0: {e}"))
        })
        .transpose()?;
    let blockers = apply_blockers(&before.identity);
    Ok(Plan {
        before,
        after_msr,
        after_mmio,
        pl1_watts,
        pl2_watts,
        blockers,
    })
}

fn same_platform(a: &Snapshot, b: &Snapshot) -> bool {
    a.identity == b.identity && a.units.raw == b.units.raw
}

fn write_backup(path: &Path, plan: &Plan) -> Result<(), String> {
    let before = &plan.before;
    let id = &before.identity;
    let mut text = String::new();
    let mut line = |key: &str, value: String| {
        let _ = writeln!(text, "{key}={value}");
    };
    line("created_utc", utc_timestamp());
    line("vendor", sanitize_label(&id.vendor));
    line("model", sanitize_label(&id.model));
    line("cpu_family", format!("0x{:x}", id.cpu_family));
    line("cpu_model", format!("0x{:x}", id.cpu_model));
    line("cpu_stepping", format!("0x{:x}", id.cpu_stepping));
    line("units", hex(before.units.raw));
    let registers = [("msr_0x610", Some(before.msr)), ("mmio_59a0", before.mmio)];
    for (name, raw) in registers {
        match raw {
            Some(raw) => {
                let limits = decode_limits(raw, before.units);
                line(name, hex(raw));
                line(&format!("{name}_pl1_w"), format_watts(limits.pl1.watts));
                line(&format!("{name}_pl2_w"), format_watts(limits.pl2.watts));
            }
            None => line(name, "unavailable".into()),
        }
    }
    line("target_pl1_w", plan.pl1_watts.to_string());
    line("target_pl2_w", plan.pl2_watts.to_string());
    line("target_msr_0x610", hex(plan.after_msr));
    line(
        "target_mmio_59a0",
        plan.after_mmio.map_or("unavailable".into(), hex),
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create new backup at {}: {e}", path.display()))?;
    file.write_all(format!("{BACKUP_HEADER}\n{text}").as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Could not fully persist backup: {e}"))
}

/// Writes MMIO then MSR, verifies both by readback and rolls back on any failure.
/// Rollback is best effort: firmware changes or a failing driver can prevent it.
pub fn apply(
    backend: &mut dyn HardwareBackend,
    plan: &Plan,
    backup: &Path,
) -> Result<Snapshot, String> {
    if !plan.blockers.is_empty() {
        return Err(format!("Apply blocked: {}", plan.blockers.join("; ")));
    }
    let (Some(before_mmio), Some(after_mmio)) = (plan.before.mmio, plan.after_mmio) else {
        return Err("Apply blocked: the MMIO register is unavailable".into());
    };
    // Detect a racing firmware or driver change between plan and apply.
    let current = read_snapshot(backend)?;
    if !same_platform(&current, &plan.before)
        || current.msr != plan.before.msr
        || current.mmio != plan.before.mmio
    {
        return Err(
            "Register values or CPU identity changed since the plan. Re-run plan; no writes \
             performed."
                .into(),
        );
    }
    write_backup(backup, plan)?;
    backend.set_audit_phase(AuditPhase::Apply);
    let action = (|| -> Result<Snapshot, String> {
        backend.write_mmio_limits(after_mmio)?;
        backend.write_msr_limits(plan.after_msr)?;
        let after = read_snapshot(backend)?;
        if !same_platform(&after, &plan.before)
            || after.msr != plan.after_msr
            || after.mmio != Some(after_mmio)
        {
            return Err("Readback mismatch. Firmware may have overwritten the limits.".into());
        }
        Ok(after)
    })();
    let problem = match action {
        Ok(after) => return Ok(after),
        Err(problem) => problem,
    };
    backend.set_audit_phase(AuditPhase::Rollback);
    // Restore the MSR first, then MMIO. Lock bits are never changed.
    let msr = rollback_register(backend, Register::Msr, plan.before.msr, plan.after_msr);
    let mmio = rollback_register(backend, Register::Mmio, before_mmio, after_mmio);
    Err(format!(
        "{problem}. Rollback MSR: {}; MMIO: {}. Re-check status immediately; backup: {}",
        rollback_text(&msr),
        rollback_text(&mmio),
        backup.display()
    ))
}

#[derive(Debug, Clone, Copy)]
enum Register {
    Msr,
    Mmio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rollback {
    NotNeeded,
    Restored,
}

fn read_register(backend: &mut dyn HardwareBackend, register: Register) -> Result<u64, String> {
    match register {
        Register::Msr => backend.read_msr_limits(),
        Register::Mmio => backend.read_mmio_limits(),
    }
}

fn write_register(
    backend: &mut dyn HardwareBackend,
    register: Register,
    value: u64,
) -> Result<(), String> {
    match register {
        Register::Msr => backend.write_msr_limits(value),
        Register::Mmio => backend.write_mmio_limits(value),
    }
}

/// Never restores a stale full register over an unexpected firmware change.
fn rollback_register(
    backend: &mut dyn HardwareBackend,
    register: Register,
    before: u64,
    attempted: u64,
) -> Result<Rollback, String> {
    let current = read_register(backend, register)?;
    if current == before {
        return Ok(Rollback::NotNeeded);
    }
    if current != attempted {
        return Err(format!(
            "skipped, unexpected value {}; inspect the hardware",
            hex(current)
        ));
    }
    write_register(backend, register, before)?;
    let actual = read_register(backend, register)?;
    if actual != before {
        return Err(format!("readback mismatch after restore ({})", hex(actual)));
    }
    Ok(Rollback::Restored)
}

fn rollback_text(result: &Result<Rollback, String>) -> String {
    match result {
        Ok(Rollback::NotNeeded) => "not needed (original value present)".into(),
        Ok(Rollback::Restored) => "restored and verified".into(),
        Err(error) => format!("FAILED ({error})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::read_log;
    use crate::backend::MockBackend;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rapl_lib_{}_{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn validates_limits_in_one_place() {
        assert!(validate_limits(55, 157, false).is_ok());
        assert!(validate_limits(4, 157, false).is_err());
        assert!(validate_limits(60, 50, true).is_err());
        assert!(validate_limits(55, 158, false).is_err());
        assert!(validate_limits(56, 157, false).is_err());
        assert!(validate_limits(157, 157, true).is_ok());
    }

    #[test]
    fn rejects_unsupported_identity() {
        let good = MockBackend::default().identity().unwrap();
        assert!(validate_identity(&good).is_ok());
        let cases = [
            Identity {
                vendor: "AuthenticAMD".into(),
                ..good.clone()
            },
            Identity {
                cpu_model: 0xbf,
                ..good.clone()
            },
            Identity {
                model: "Intel(R) Core(TM) i7-13650HX".into(),
                ..good.clone()
            },
            Identity {
                cpu_supported: false,
                ..good.clone()
            },
            Identity {
                packages: 2,
                ..good.clone()
            },
        ];
        for identity in cases {
            assert!(validate_identity(&identity).is_err(), "{identity:?}");
        }
    }

    #[test]
    fn plan_reports_watt_columns_in_write_order() {
        let mut dev = MockBackend::default();
        let plan = build_plan(&mut dev, 100, 157, true).unwrap();
        let changes = plan.changes();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].register, REGISTER_MMIO_PKG_LIMIT);
        assert_eq!(changes[1].register, REGISTER_MSR_PKG_LIMIT);
        for change in &changes {
            assert_eq!(change.watts.pl1_before_w, 55.0);
            assert_eq!(change.watts.pl2_before_w, 157.0);
            assert_eq!(change.watts.pl1_set_w, 100.0);
            assert_eq!(change.watts.pl2_set_w, 157.0);
        }
        assert!(!plan.already_applied());
        assert!(build_plan(&mut dev, 55, 157, false)
            .unwrap()
            .already_applied());
    }

    #[test]
    fn mock_plan_and_apply() {
        let mut dev = MockBackend::default();
        let plan = build_plan(&mut dev, 50, 120, false).unwrap();
        let path = temp("apply.txt");
        let result = apply(&mut dev, &plan, &path).unwrap();
        assert_eq!(result.msr, plan.after_msr);
        assert_eq!(result.mmio, plan.after_mmio);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("WIN_MMIO_MSR_BACKUP_V2\n"));
        assert!(contents.contains("\nmsr_0x610_pl1_w=55\n"));
        assert!(contents.contains("\ntarget_pl2_w=120\n"));
        let again = build_plan(&mut dev, 55, 157, false).unwrap();
        let error = apply(&mut dev, &again, &path).unwrap_err();
        assert!(error.contains("Cannot create new backup"), "{error}");
        assert_eq!(dev.msr, plan.after_msr, "no write without a new backup");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sustained_pl1_needs_explicit_acknowledgment() {
        let mut dev = MockBackend::default();
        assert!(build_plan(&mut dev, 157, 157, false).is_err());
        assert!(build_plan(&mut dev, 157, 157, true).is_ok());
    }

    #[test]
    fn status_works_without_mmio_but_apply_is_blocked() {
        let mut dev = MockBackend::default();
        dev.mmio_readable = false;
        dev.mmio_verified = false;
        let snapshot = read_snapshot(&mut dev).unwrap();
        assert_eq!(snapshot.mmio, None);
        let plan = build_plan(&mut dev, 50, 157, false).unwrap();
        assert_eq!(plan.changes().len(), 1);
        assert_eq!(plan.blockers().len(), 2);
        assert!(!plan.already_applied());
        let path = temp("blocked.txt");
        let error = apply(&mut dev, &plan, &path).unwrap_err();
        assert!(error.starts_with("Apply blocked"), "{error}");
        assert!(!path.exists());
    }

    #[test]
    fn read_only_driver_blocks_apply() {
        let mut dev = MockBackend::default();
        dev.live_writes_enabled = false;
        let before = dev.msr;
        let plan = build_plan(&mut dev, 50, 157, false).unwrap();
        assert_eq!(plan.blockers().len(), 1);
        assert!(apply(&mut dev, &plan, &temp("readonly.txt")).is_err());
        assert_eq!(dev.msr, before);
    }

    #[test]
    fn detects_race_between_plan_and_apply() {
        let mut dev = MockBackend::default();
        let plan = build_plan(&mut dev, 50, 157, false).unwrap();
        dev.msr ^= 1 << 20;
        let path = temp("race.txt");
        let error = apply(&mut dev, &plan, &path).unwrap_err();
        assert!(error.contains("changed since the plan"), "{error}");
        assert!(!path.exists());
    }

    #[test]
    fn rollback_restores_both_registers_and_is_audited() {
        let log = temp("rollback.jsonl");
        let mut dev =
            MockBackend::default().with_audit_log(Some(log.clone()), Some("unit test".into()));
        let old_msr = dev.msr;
        let old_mmio = dev.mmio;
        let plan = build_plan(&mut dev, 100, 157, true).unwrap();
        dev.fail_next_msr_write = true;
        let path = temp("rollback.txt");
        let error = apply(&mut dev, &plan, &path).unwrap_err();
        assert!(error.contains("Rollback MSR: not needed"), "{error}");
        assert!(error.contains("MMIO: restored and verified"), "{error}");
        assert_eq!(dev.msr, old_msr);
        assert_eq!(dev.mmio, old_mmio);
        let rows = read_log(std::fs::read(&log).unwrap().as_slice(), None)
            .unwrap()
            .rows;
        let summary: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.register.clone().unwrap(),
                    row.phase.clone().unwrap(),
                    row.outcome.clone().unwrap(),
                )
            })
            .collect();
        let expected = [
            (REGISTER_MMIO_PKG_LIMIT, "apply", "pending"),
            (REGISTER_MMIO_PKG_LIMIT, "apply", "changed"),
            (REGISTER_MSR_PKG_LIMIT, "apply", "pending"),
            (REGISTER_MSR_PKG_LIMIT, "apply", "driver_error"),
            (REGISTER_MMIO_PKG_LIMIT, "rollback", "pending"),
            (REGISTER_MMIO_PKG_LIMIT, "rollback", "changed"),
        ]
        .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()));
        assert_eq!(summary, expected);
        let restore = &rows[5];
        assert_eq!(restore.pl1_before_w, Some(100.0));
        assert_eq!(restore.pl1_set_w, Some(55.0));
        assert_eq!(restore.reason.as_deref(), Some("unit test"));
        assert!(restore.simulated);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&log);
    }
}
