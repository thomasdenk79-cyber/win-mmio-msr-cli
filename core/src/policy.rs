//! The single PL1/PL2 policy shared by the CLI, the watchdog and the bridge.
//! The kernel driver mirrors these limits in `driver/include/rapl_ioctl.h`.
use crate::rapl::{decode_limits, Units, LOCK_BIT, POWER_FIELD_MASK};
use std::fmt;

/// Lowest PL1/PL2 the tools ever program.
pub const MIN_WATTS: u16 = 5;
/// Intel processor base power of the i9-13950HX.
pub const PROCESSOR_BASE_WATTS: u16 = 55;
/// Intel maximum turbo power of the i9-13950HX.
pub const MAX_TURBO_WATTS: u16 = 157;

/// Request-level check: `MIN_WATTS <= PL1 <= PL2 <= MAX_TURBO_WATTS`. Any PL1 and PL2 in
/// this range is accepted, including a sustained PL1 above the processor base power.
pub fn validate_limits(pl1_watts: u16, pl2_watts: u16) -> Result<(), String> {
    if pl1_watts < MIN_WATTS || pl2_watts > MAX_TURBO_WATTS || pl2_watts < pl1_watts {
        return Err(format!(
            "Use {MIN_WATTS} <= PL1 <= PL2 <= {MAX_TURBO_WATTS} W (got PL1 {pl1_watts} W, PL2 {pl2_watts} W)"
        ));
    }
    Ok(())
}

/// Reason why a raw register write is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteViolation {
    Locked,
    ProtectedBitsChanged,
    LimitDisabled,
    OutOfRange,
}

impl fmt::Display for WriteViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => write!(f, "the register is locked by firmware"),
            Self::ProtectedBitsChanged => write!(
                f,
                "only the PL1/PL2 power and enable fields may change (mask 0x0000ffff0000ffff)"
            ),
            Self::LimitDisabled => write!(f, "PL1 and PL2 must both stay enabled"),
            Self::OutOfRange => write!(
                f,
                "the value must satisfy {MIN_WATTS} <= PL1 <= PL2 <= {MAX_TURBO_WATTS} W"
            ),
        }
    }
}

/// Register-level write policy: the register is not locked, only PL1/PL2 power and
/// enable fields change, both limits stay enabled and the decoded watts satisfy
/// `MIN_WATTS <= PL1 <= PL2 <= MAX_TURBO_WATTS`. Rollbacks follow the same rule, so a
/// firmware value outside it is never restored (the lower limit stays active).
pub fn check_register_write(
    current: u64,
    requested: u64,
    units: Units,
) -> Result<(), WriteViolation> {
    if current & LOCK_BIT != 0 {
        return Err(WriteViolation::Locked);
    }
    if (current ^ requested) & !POWER_FIELD_MASK != 0 {
        return Err(WriteViolation::ProtectedBitsChanged);
    }
    let limits = decode_limits(requested, units);
    if !(limits.pl1.enabled && limits.pl2.enabled) {
        return Err(WriteViolation::LimitDisabled);
    }
    let (min, max) = (f64::from(MIN_WATTS), f64::from(MAX_TURBO_WATTS));
    if limits.pl1.watts < min || limits.pl1.watts > limits.pl2.watts || limits.pl2.watts > max {
        return Err(WriteViolation::OutOfRange);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rapl::with_power_limits;

    const UNITS: u64 = 0x000a_0e03;

    #[test]
    fn validates_limits_in_one_place() {
        assert!(validate_limits(55, 157).is_ok());
        assert!(validate_limits(56, 157).is_ok());
        assert!(validate_limits(157, 157).is_ok());
        assert!(validate_limits(4, 157).is_err());
        assert!(validate_limits(60, 50).is_err());
        assert!(validate_limits(55, 158).is_err());
    }

    #[test]
    fn register_writes_follow_the_policy() {
        let units = Units::decode(UNITS);
        let base = (0x6eu64 << 17) | (1 << 16) | (0x21u64 << 49) | (1 << 48);
        let current = with_power_limits(base, units, 55, 157).unwrap();
        let target = with_power_limits(current, units, 45, 100).unwrap();
        assert_eq!(check_register_write(current, target, units), Ok(()));
        assert_eq!(
            check_register_write(current | LOCK_BIT, target | LOCK_BIT, units),
            Err(WriteViolation::Locked)
        );
        assert_eq!(
            check_register_write(current, target ^ (1 << 17), units),
            Err(WriteViolation::ProtectedBitsChanged)
        );
        assert_eq!(
            check_register_write(current, target & !(1 << 47), units),
            Err(WriteViolation::LimitDisabled)
        );
        // PL1 above PL2, PL2 above the maximum and PL1 below the minimum.
        let swapped = (target & !0x7fff) | (110 * 8);
        let too_high = (target & !(0x7fff << 32)) | ((158 * 8) << 32);
        let too_low = (target & !0x7fff) | (4 * 8);
        for value in [swapped, too_high, too_low] {
            assert_eq!(
                check_register_write(current, value, units),
                Err(WriteViolation::OutOfRange),
                "{value:#x}"
            );
        }
    }
}
