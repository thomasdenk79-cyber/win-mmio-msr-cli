//! High-level, fail-closed orchestration for Intel RAPL package limits.
pub mod backend;
pub use rapl_core::rapl;

use backend::{HardwareBackend, Identity};
use rapl::{decode_limits, with_power_limits, Units};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub const MAX_TURBO_WATTS: u16 = 157;
pub const PROCESSOR_BASE_WATTS: u16 = 55;
pub const MIN_WATTS: u16 = 5;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Snapshot {
    pub identity: Identity,
    pub units: Units,
    pub msr: u64,
    pub mmio: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Plan {
    before: Snapshot,
    after_msr: u64,
    after_mmio: u64,
    pl1_watts: u16,
    pl2_watts: u16,
}

impl Plan {
    pub fn before(&self) -> &Snapshot {
        &self.before
    }
    pub fn targets(&self) -> (u64, u64) {
        (self.after_msr, self.after_mmio)
    }
    pub fn watts(&self) -> (u16, u16) {
        (self.pl1_watts, self.pl2_watts)
    }
}

fn validate_identity(id: &Identity) -> Result<(), String> {
    if id.vendor != "GenuineIntel" {
        return Err(format!("Refusing unsupported vendor: {}", id.vendor));
    }
    if !id.model.contains("i9-13950HX") {
        return Err(format!(
            "Only the validated Intel i9-13950HX profile is supported: {}",
            id.model
        ));
    }
    if !id.mmio_verified {
        return Err("Bridge has NOT verified MCHBAR MMIO register layout/address".into());
    }
    Ok(())
}

pub fn read_snapshot(backend: &mut dyn HardwareBackend) -> Result<Snapshot, String> {
    let identity = backend.identity()?;
    validate_identity(&identity)?;
    let raw_units = backend.read_units()?;
    if raw_units == u64::MAX {
        return Err("Implausible RAPL units (all bits set)".into());
    }
    let units = Units::decode(raw_units);
    let msr = backend.read_msr_limits()?;
    let mmio = backend.read_mmio_limits()?;
    // A bridge must return a coherent 64-bit RAPL register from 0x59a0/0x59a4.
    // It must not guess the MCHBAR base or silently substitute a different register.
    if msr == u64::MAX || mmio == u64::MAX {
        return Err("Implausible register value (all bits set)".into());
    }
    Ok(Snapshot {
        identity,
        units,
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
    if pl1_watts < MIN_WATTS || pl2_watts > MAX_TURBO_WATTS || pl2_watts < pl1_watts {
        return Err(format!(
            "Use {MIN_WATTS} <= PL1 <= PL2 <= {MAX_TURBO_WATTS} W"
        ));
    }
    if pl1_watts > PROCESSOR_BASE_WATTS && !allow_sustained_above_base {
        return Err(format!(
            "PL1={pl1_watts} W exceeds Intel's {PROCESSOR_BASE_WATTS} W base power. \
             Provide --allow-sustained-above-base to acknowledge this."
        ));
    }
    let before = read_snapshot(backend)?;
    let after_msr = with_power_limits(before.msr, before.units, pl1_watts, pl2_watts)
        .map_err(|e| format!("MSR 0x610: {e}"))?;
    let after_mmio = with_power_limits(before.mmio, before.units, pl1_watts, pl2_watts)
        .map_err(|e| format!("MMIO MCHBAR+0x59a0: {e}"))?;
    Ok(Plan {
        before,
        after_msr,
        after_mmio,
        pl1_watts,
        pl2_watts,
    })
}

pub fn describe_snapshot(s: &Snapshot) -> String {
    let msr = decode_limits(s.msr, s.units);
    let mmio = decode_limits(s.mmio, s.units);
    format!(
        "CPU: {} / {}{}\n\
         RAPL unit: {:.8} W, {:.8} s\n\
         MSR 0x610: 0x{:016x}  PL1 {:.3} W [{}]  PL2 {:.3} W [{}]  locked={}\n\
         MMIO MCHBAR+0x59a0: 0x{:016x}  PL1 {:.3} W [{}]  PL2 {:.3} W [{}]  locked={}\n",
        s.identity.vendor,
        s.identity.model,
        if s.identity.simulated {
            " [MOCK, NO HARDWARE]"
        } else {
            ""
        },
        s.units.watts_per_unit,
        s.units.seconds_per_unit,
        s.msr,
        msr.pl1.watts,
        enabled(msr.pl1.enabled),
        msr.pl2.watts,
        enabled(msr.pl2.enabled),
        msr.locked,
        s.mmio,
        mmio.pl1.watts,
        enabled(mmio.pl1.enabled),
        mmio.pl2.watts,
        enabled(mmio.pl2.enabled),
        mmio.locked,
    )
}

fn enabled(on: bool) -> &'static str {
    if on {
        "enabled"
    } else {
        "disabled"
    }
}

pub fn describe_plan(p: &Plan) -> String {
    let new_msr = decode_limits(p.after_msr, p.before.units);
    let new_mmio = decode_limits(p.after_mmio, p.before.units);
    format!(
        "{}\
         New values: PL1={} W, PL2={} W\n\
         MSR:  0x{:016x} -> 0x{:016x} ({:.3} W / {:.3} W)\n\
         MMIO: 0x{:016x} -> 0x{:016x} ({:.3} W / {:.3} W)\n\
         Time windows, clamp/lock and reserved bits are preserved.\n\
         Note: BIOS/EC/DTT and other platform limits can still override these settings.\n",
        describe_snapshot(&p.before),
        p.pl1_watts,
        p.pl2_watts,
        p.before.msr,
        p.after_msr,
        new_msr.pl1.watts,
        new_msr.pl2.watts,
        p.before.mmio,
        p.after_mmio,
        new_mmio.pl1.watts,
        new_mmio.pl2.watts,
    )
}

fn write_backup(path: &Path, p: &Plan) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create new backup at {}: {e}", path.display()))?;
    writeln!(file, "WIN_MMIO_MSR_BACKUP_V1")
        .and_then(|_| writeln!(file, "vendor={}", p.before.identity.vendor))
        .and_then(|_| writeln!(file, "model={}", p.before.identity.model))
        .and_then(|_| writeln!(file, "units=0x{:016x}", p.before.units.raw))
        .and_then(|_| writeln!(file, "msr=0x{:016x}", p.before.msr))
        .and_then(|_| writeln!(file, "mmio=0x{:016x}", p.before.mmio))
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Could not fully persist backup: {e}"))
}

/// Writes MMIO then MSR, re-reads both, and attempts to rollback on any failure.
/// Rollback is best-effort: BIOS/firmware changes or a failing driver can prevent it.
pub fn apply(
    backend: &mut dyn HardwareBackend,
    plan: &Plan,
    backup: &Path,
) -> Result<Snapshot, String> {
    // Detect a racing firmware change between plan and apply.
    let current = read_snapshot(backend)?;
    if current.identity.model != plan.before.identity.model
        || current.identity.vendor != plan.before.identity.vendor
        || current.identity.simulated != plan.before.identity.simulated
        || current.units.raw != plan.before.units.raw
        || current.msr != plan.before.msr
        || current.mmio != plan.before.mmio
    {
        return Err(
            "Register values/CPU changed since plan. Re-run plan; no writes performed.".into(),
        );
    }
    write_backup(backup, plan)?;
    let action = (|| -> Result<Snapshot, String> {
        backend.write_mmio_limits(plan.after_mmio)?;
        backend.write_msr_limits(plan.after_msr)?;
        let after = read_snapshot(backend)?;
        if after.units.raw != plan.before.units.raw
            || after.identity.model != plan.before.identity.model
            || after.identity.simulated != plan.before.identity.simulated
            || after.msr != plan.after_msr
            || after.mmio != plan.after_mmio
        {
            return Err("Readback mismatch. Firmware may have overwritten limits.".into());
        }
        Ok(after)
    })();
    match action {
        Ok(value) => Ok(value),
        Err(problem) => {
            // Restore the lower-priority MSR first, then MMIO. No locks are changed.
            let msr_rollback = rollback_register(backend, plan.before.msr, plan.after_msr, false);
            let mmio_rollback = rollback_register(backend, plan.before.mmio, plan.after_mmio, true);
            Err(format!(
                "{problem}. Rollback MSR={msr_rollback:?}; MMIO={mmio_rollback:?}. \
                 Re-check status immediately; backup: {}",
                backup.display()
            ))
        }
    }
}

/// Never restore a stale full register over an unexpected firmware change.
fn rollback_register(
    backend: &mut dyn HardwareBackend,
    before: u64,
    attempted: u64,
    mmio: bool,
) -> Result<(), String> {
    let current = if mmio {
        backend.read_mmio_limits()?
    } else {
        backend.read_msr_limits()?
    };
    if current == before {
        return Ok(());
    }
    if current != attempted {
        return Err("Rollback skipped: unexpected register change; inspect hardware".into());
    }
    if mmio {
        backend.write_mmio_limits(before)?;
    } else {
        backend.write_msr_limits(before)?;
    }
    let actual = if mmio {
        backend.read_mmio_limits()?
    } else {
        backend.read_msr_limits()?
    };
    if actual != before {
        return Err("Rollback readback mismatch".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;
    use crate::rapl::power_fields_equal;

    #[test]
    fn mock_plan_and_apply() {
        let mut dev = MockBackend::default();
        let plan = build_plan(&mut dev, 55, 157, false).unwrap();
        let path =
            std::env::temp_dir().join(format!("rapl_test_{}_{}.txt", std::process::id(), "apply"));
        let _ = std::fs::remove_file(&path);
        let result = apply(&mut dev, &plan, &path).unwrap();
        assert!(power_fields_equal(result.msr, plan.after_msr));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("WIN_MMIO_MSR_BACKUP_V1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sustained_pl1_needs_explicit_acknowledgment() {
        let mut dev = MockBackend::default();
        assert!(build_plan(&mut dev, 157, 157, false).is_err());
        assert!(build_plan(&mut dev, 157, 157, true).is_ok());
    }

    #[test]
    fn rollback_restores_both_registers() {
        let mut dev = MockBackend::default();
        let old_msr = dev.msr;
        let old_mmio = dev.mmio;
        let plan = build_plan(&mut dev, 100, 157, true).unwrap();
        dev.fail_next_msr_write = true;
        let path = std::env::temp_dir().join(format!(
            "rapl_test_{}_{}.txt",
            std::process::id(),
            "rollback"
        ));
        let _ = std::fs::remove_file(&path);
        assert!(apply(&mut dev, &plan, &path).is_err());
        assert_eq!(dev.msr, old_msr);
        assert_eq!(dev.mmio, old_mmio);
        let _ = std::fs::remove_file(&path);
    }
}
