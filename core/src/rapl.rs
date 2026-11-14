//! Intel package RAPL power units and MSR 0x610 register bitfields.
//! MMIO can share this layout only after platform-specific verification.
use std::fmt;

pub const MSR_RAPL_POWER_UNIT: u32 = 0x606;
pub const MSR_PKG_POWER_LIMIT: u32 = 0x610;
pub const MMIO_PKG_POWER_LIMIT_OFFSET: u32 = 0x59a0;

const LIMIT_MASK: u64 = 0x7fff;
const PL1_ENABLE: u64 = 1 << 15;
const PL2_ENABLE: u64 = 1 << 47;
pub const LOCK_BIT: u64 = 1 << 63;
const CHANGE_MASK: u64 = LIMIT_MASK | (LIMIT_MASK << 32) | PL1_ENABLE | PL2_ENABLE;
/// Bits a guarded write may change (PL1/PL2 power and enable). Equals the driver's
/// `RAPL_POWER_FIELD_MASK`.
pub const POWER_FIELD_MASK: u64 = CHANGE_MASK;
const _: () = assert!(POWER_FIELD_MASK == 0x0000_ffff_0000_ffff);

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Units {
    pub raw: u64,
    pub watts_per_unit: f64,
    pub seconds_per_unit: f64,
}

impl Units {
    pub fn decode(raw: u64) -> Self {
        let power_exp = (raw & 0xf) as i32;
        let time_exp = ((raw >> 16) & 0xf) as i32;
        Self {
            raw,
            watts_per_unit: 2f64.powi(-power_exp),
            seconds_per_unit: 2f64.powi(-time_exp),
        }
    }

    fn encode_watts(&self, watts: u16) -> Result<u64, RaplError> {
        let units = (f64::from(watts) / self.watts_per_unit).round();
        if !units.is_finite() || !(0.0..=LIMIT_MASK as f64).contains(&units) {
            return Err(RaplError::OutOfRange(watts));
        }
        let encoded = units as u64;
        let represented = encoded as f64 * self.watts_per_unit;
        if (represented - f64::from(watts)).abs() > self.watts_per_unit / 2.0 + 1e-9 {
            return Err(RaplError::NotRepresentable(watts));
        }
        Ok(encoded)
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Limit {
    pub watts: f64,
    pub enabled: bool,
    pub clamp: bool,
    /// Raw 7-bit time-window code (Y in bits 0-4, Z in bits 5-6).
    pub time_window: u8,
    /// Decoded time window: 2^Y * (1 + Z/4) * time unit.
    pub time_window_s: f64,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct DecodedLimits {
    pub pl1: Limit,
    pub pl2: Limit,
    pub locked: bool,
}

#[derive(Debug)]
pub enum RaplError {
    Locked,
    OutOfRange(u16),
    NotRepresentable(u16),
}

impl fmt::Display for RaplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => write!(f, "package power limit register is locked"),
            Self::OutOfRange(watts) => write!(f, "{watts} W is not encodable in 15 bits"),
            Self::NotRepresentable(watts) => write!(f, "{watts} W cannot be represented precisely"),
        }
    }
}

impl std::error::Error for RaplError {}

pub fn decode_limits(raw: u64, units: Units) -> DecodedLimits {
    let pl1_window = ((raw >> 17) & 0x7f) as u8;
    let pl2_window = ((raw >> 49) & 0x7f) as u8;
    DecodedLimits {
        pl1: Limit {
            watts: (raw & LIMIT_MASK) as f64 * units.watts_per_unit,
            enabled: raw & PL1_ENABLE != 0,
            clamp: raw & (1 << 16) != 0,
            time_window: pl1_window,
            time_window_s: time_window_seconds(pl1_window, units),
        },
        pl2: Limit {
            watts: ((raw >> 32) & LIMIT_MASK) as f64 * units.watts_per_unit,
            enabled: raw & PL2_ENABLE != 0,
            clamp: raw & (1 << 48) != 0,
            time_window: pl2_window,
            time_window_s: time_window_seconds(pl2_window, units),
        },
        locked: raw & LOCK_BIT != 0,
    }
}

/// Intel SDM: time window = 2^Y * (1.0 + Z / 4.0) * time unit.
pub fn time_window_seconds(code: u8, units: Units) -> f64 {
    let y = i32::from(code & 0x1f);
    let z = f64::from((code >> 5) & 0x3);
    2f64.powi(y) * (1.0 + z / 4.0) * units.seconds_per_unit
}

/// Read-modify-write: update ONLY PL1/PL2 numeric fields and the enable bits.
/// Preserve clamp, time windows, lock, all reserved fields and other controls.
pub fn with_power_limits(
    original: u64,
    units: Units,
    pl1_watts: u16,
    pl2_watts: u16,
) -> Result<u64, RaplError> {
    if original & LOCK_BIT != 0 {
        return Err(RaplError::Locked);
    }
    let pl1 = units.encode_watts(pl1_watts)?;
    let pl2 = units.encode_watts(pl2_watts)?;
    Ok((original & !CHANGE_MASK) | pl1 | (pl2 << 32) | PL1_ENABLE | PL2_ENABLE)
}

/// Compare only fields this tool intentionally changes. Firmware may change other fields.
pub fn power_fields_equal(a: u64, b: u64) -> bool {
    (a & CHANGE_MASK) == (b & CHANGE_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_decode() {
        let u = Units::decode(3 | (10 << 16));
        assert_eq!(u.watts_per_unit, 0.125);
        assert_eq!(u.seconds_per_unit, 1.0 / 1024.0);
    }

    #[test]
    fn encode_55_and_157() {
        let u = Units::decode(3);
        let old = (0x15u64 << 17) | (0x12u64 << 49) | (1 << 16) | (1 << 48);
        let new = with_power_limits(old, u, 55, 157).unwrap();
        let decoded = decode_limits(new, u);
        assert_eq!(decoded.pl1.watts, 55.0);
        assert_eq!(decoded.pl2.watts, 157.0);
        assert!(decoded.pl1.enabled && decoded.pl2.enabled);
        assert_eq!(new & !CHANGE_MASK, old & !CHANGE_MASK);
    }

    #[test]
    fn refuses_locked_register() {
        assert!(matches!(
            with_power_limits(1 << 63, Units::decode(3), 55, 157),
            Err(RaplError::Locked)
        ));
    }

    #[test]
    fn refuses_unrepresentable_number() {
        assert!(with_power_limits(0, Units::decode(0), 55, 157).is_ok());
        assert!(matches!(
            with_power_limits(0, Units::decode(15), 55, 157),
            Err(RaplError::OutOfRange(_))
        ));
        assert!(matches!(
            with_power_limits(0, Units::decode(15), 500, 501),
            Err(RaplError::OutOfRange(_))
        ));
    }

    #[test]
    fn comparison_only_considers_written_fields() {
        let a = with_power_limits(0, Units::decode(3), 55, 157).unwrap();
        assert!(power_fields_equal(a, a ^ (1 << 20)));
        assert!(!power_fields_equal(a, a ^ 1));
    }

    #[test]
    fn decodes_time_windows_in_seconds() {
        // Typical Raptor Lake units: 1/8 W and 2^-10 s; PL1 tau 28 s = 2^14 * 1.75 / 1024.
        let units = Units::decode(0x000a_0e03);
        let raw = (0x6eu64 << 17) | (0x21u64 << 49);
        let decoded = decode_limits(raw, units);
        assert_eq!(decoded.pl1.time_window, 0x6e);
        assert_eq!(decoded.pl1.time_window_s, 28.0);
        assert_eq!(decoded.pl2.time_window_s, 2.0 * 1.25 / 1024.0);
    }
}
