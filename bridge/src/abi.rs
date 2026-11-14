//! Binary interface of the RaplPkg driver. Mirrors `driver/include/rapl_ioctl.h`;
//! the IOCTL codes and structure sizes are asserted on both sides.
use rapl_core::audit::{REGISTER_MMIO_PKG_LIMIT, REGISTER_MSR_PKG_LIMIT, REGISTER_MSR_UNITS};
use std::mem::size_of;

pub const DEVICE_PATH: &str = r"\\.\RaplPkg";
/// Version of the request, response and info structures.
pub const ABI_VERSION: u32 = 2;

const DEVICE_TYPE: u32 = 0x8010;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_ACCESS: u32 = 1;
const FILE_WRITE_ACCESS: u32 = 2;

const fn ctl_code(function: u32, access: u32) -> u32 {
    (DEVICE_TYPE << 16) | (access << 14) | (function << 2) | METHOD_BUFFERED
}

pub const IOCTL_GET_INFO: u32 = ctl_code(0x800, FILE_READ_ACCESS);
pub const IOCTL_READ_MSR: u32 = ctl_code(0x801, FILE_READ_ACCESS);
pub const IOCTL_READ_MMIO: u32 = ctl_code(0x802, FILE_READ_ACCESS);
pub const IOCTL_WRITE_MSR: u32 = ctl_code(0x803, FILE_WRITE_ACCESS);
pub const IOCTL_WRITE_MMIO: u32 = ctl_code(0x804, FILE_WRITE_ACCESS);

/// The MCHBAR+0x59A0 layout was validated for this platform (build-time gate).
pub const FLAG_MMIO_VERIFIED: u32 = 0x1;
/// Live writes are disabled (build-time gate).
pub const FLAG_READ_ONLY: u32 = 0x2;
/// GenuineIntel, family 6, model 0xB7 and a single package.
pub const FLAG_CPU_SUPPORTED: u32 = 0x4;
/// The driver can read MCHBAR+0x59A0 (build-time gate).
pub const FLAG_MMIO_READABLE: u32 = 0x8;

/// Vendor code the driver reports for the CPUID vendor string "GenuineIntel".
pub const VENDOR_INTEL: u32 = 0x8086;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RegisterId {
    Units = 1,
    PackageLimit = 2,
    MmioPackageLimit = 3,
}

impl RegisterId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Units => REGISTER_MSR_UNITS,
            Self::PackageLimit => REGISTER_MSR_PKG_LIMIT,
            Self::MmioPackageLimit => REGISTER_MMIO_PKG_LIMIT,
        }
    }

    pub fn read_ioctl(self) -> u32 {
        match self {
            Self::Units | Self::PackageLimit => IOCTL_READ_MSR,
            Self::MmioPackageLimit => IOCTL_READ_MMIO,
        }
    }

    /// Bridge command and IOCTL of a writable register; `None` for MSR 0x606.
    pub fn write_operation(self) -> Option<(&'static str, u32)> {
        match self {
            Self::Units => None,
            Self::PackageLimit => Some(("msr-write", IOCTL_WRITE_MSR)),
            Self::MmioPackageLimit => Some(("mmio-write", IOCTL_WRITE_MMIO)),
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub size: u32,
    pub version: u32,
    pub register_id: u32,
    pub reserved: u32,
    /// Writes only: the driver refuses when the register no longer holds this value.
    pub expected: u64,
    pub value: u64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    pub size: u32,
    pub version: u32,
    pub register_id: u32,
    pub flags: u32,
    pub value: u64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub size: u32,
    pub version: u32,
    pub vendor: u32,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub flags: u32,
    pub packages: u32,
    /// Physical MCHBAR base, or 0 when MMIO reads are disabled.
    pub mchbar_base: u64,
}

const _: () = assert!(size_of::<Request>() == 32);
const _: () = assert!(size_of::<Response>() == 24);
const _: () = assert!(size_of::<Info>() == 40);

impl Request {
    pub fn new(register: RegisterId, expected: u64, value: u64) -> Self {
        Self {
            size: size_of::<Self>() as u32,
            version: ABI_VERSION,
            register_id: register as u32,
            reserved: 0,
            expected,
            value,
        }
    }
}

impl Response {
    pub fn value_for(&self, register: RegisterId) -> Result<u64, String> {
        if self.size as usize != size_of::<Self>()
            || self.version != ABI_VERSION
            || self.register_id != register as u32
        {
            return Err("The driver returned an invalid response".into());
        }
        Ok(self.value)
    }
}

impl Info {
    pub fn validated(self) -> Result<Self, String> {
        if self.size as usize != size_of::<Self>() || self.version != ABI_VERSION {
            return Err(format!(
                "Driver ABI mismatch (version {}, size {}); expected version {ABI_VERSION}",
                self.version, self.size
            ));
        }
        Ok(self)
    }
}

/// Explains the Win32 errors that the driver's NTSTATUS codes map to
/// (table in `driver/README.md`).
pub fn describe_win32_error(code: u32) -> String {
    let hint = match code {
        2 | 3 => "the RaplPkg device does not exist (driver not installed or not started)",
        5 => "access denied (run as Administrator or SYSTEM)",
        50 => {
            "not supported by the driver (unsupported CPU, MCHBAR unavailable or \
             build-time gate disabled)"
        }
        87 => {
            "the driver rejected the request (invalid register or value, or write policy \
             violation)"
        }
        1117 => "the CPU or hypervisor refused the register access",
        1237 => "the register changed concurrently (expected value mismatch); re-run plan",
        1306 => "driver and bridge protocol versions differ; install matching builds",
        _ => return format!("Win32 error {code}"),
    };
    format!("{hint} (Win32 error {code})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_codes_match_the_driver_header() {
        assert_eq!(IOCTL_GET_INFO, 0x8010_6000);
        assert_eq!(IOCTL_READ_MSR, 0x8010_6004);
        assert_eq!(IOCTL_READ_MMIO, 0x8010_6008);
        assert_eq!(IOCTL_WRITE_MSR, 0x8010_a00c);
        assert_eq!(IOCTL_WRITE_MMIO, 0x8010_a010);
    }

    #[test]
    fn validates_responses() {
        let good = Response {
            size: 24,
            version: ABI_VERSION,
            register_id: RegisterId::PackageLimit as u32,
            flags: 0,
            value: 7,
        };
        assert_eq!(good.value_for(RegisterId::PackageLimit), Ok(7));
        assert!(good.value_for(RegisterId::Units).is_err());
        assert!(Response { size: 32, ..good }
            .value_for(RegisterId::PackageLimit)
            .is_err());
        assert!(Response { version: 1, ..good }
            .value_for(RegisterId::PackageLimit)
            .is_err());
        let info = Info {
            size: 40,
            version: ABI_VERSION,
            ..Info::default()
        };
        assert!(info.validated().is_ok());
        assert!(Info { version: 1, ..info }.validated().is_err());
        assert!(Info { size: 36, ..info }.validated().is_err());
    }

    #[test]
    fn explains_driver_errors() {
        assert!(describe_win32_error(1237).contains("re-run plan"));
        assert!(describe_win32_error(5).contains("access denied"));
        assert!(describe_win32_error(1306).contains("protocol versions differ"));
        assert_eq!(describe_win32_error(1), "Win32 error 1");
    }

    const DRIVER_HEADER: &str = include_str!("../../driver/include/rapl_ioctl.h");
    const DRIVER_SOURCE: &str = include_str!("../../driver/src/driver.c");

    /// Numeric `#define NAME VALUE` lines and `Name = VALUE` enum entries of the header.
    fn header_constants() -> std::collections::HashMap<&'static str, u64> {
        let parse = |text: &str| {
            let digits = text
                .trim_end_matches(',')
                .trim_end_matches(['u', 'U', 'l', 'L']);
            match digits
                .strip_prefix("0x")
                .or_else(|| digits.strip_prefix("0X"))
            {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => digits.parse().ok(),
            }
        };
        DRIVER_HEADER
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let (name, value) = match words.next()? {
                    "#define" => (words.next()?, words.next()?),
                    name if name.starts_with("RaplRegister") => (
                        words.next().filter(|w| *w == "=").map(|_| name)?,
                        words.next()?,
                    ),
                    _ => return None,
                };
                Some((name, parse(value)?))
            })
            .collect()
    }

    #[test]
    fn driver_header_matches_the_rust_contract() {
        use rapl_core::policy::{MAX_TURBO_WATTS, MIN_WATTS};
        use rapl_core::rapl::{self, decode_limits, Units};

        let c = header_constants();
        let get = |name: &str| *c.get(name).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(get("RAPL_PROTOCOL_VERSION"), u64::from(ABI_VERSION));
        assert_eq!(get("RAPL_DEVICE_TYPE"), u64::from(DEVICE_TYPE));
        let base = get("RAPL_IOCTL_BASE") as u32;
        assert_eq!(IOCTL_GET_INFO, ctl_code(base, FILE_READ_ACCESS));
        assert_eq!(IOCTL_READ_MSR, ctl_code(base + 1, FILE_READ_ACCESS));
        assert_eq!(IOCTL_READ_MMIO, ctl_code(base + 2, FILE_READ_ACCESS));
        assert_eq!(IOCTL_WRITE_MSR, ctl_code(base + 3, FILE_WRITE_ACCESS));
        assert_eq!(IOCTL_WRITE_MMIO, ctl_code(base + 4, FILE_WRITE_ACCESS));
        for (name, flag) in [
            ("RAPL_FLAG_MMIO_VERIFIED", FLAG_MMIO_VERIFIED),
            ("RAPL_FLAG_READ_ONLY", FLAG_READ_ONLY),
            ("RAPL_FLAG_CPU_SUPPORTED", FLAG_CPU_SUPPORTED),
            ("RAPL_FLAG_MMIO_READABLE", FLAG_MMIO_READABLE),
            ("RAPL_VENDOR_INTEL", VENDOR_INTEL),
            ("RAPL_MSR_POWER_UNIT", rapl::MSR_RAPL_POWER_UNIT),
            ("RAPL_MSR_PKG_POWER_LIMIT", rapl::MSR_PKG_POWER_LIMIT),
            (
                "RAPL_MMIO_PKG_POWER_LIMIT",
                rapl::MMIO_PKG_POWER_LIMIT_OFFSET,
            ),
            ("RAPL_MIN_WATTS", u32::from(MIN_WATTS)),
            ("RAPL_MAX_TURBO_WATTS", u32::from(MAX_TURBO_WATTS)),
            ("RaplRegisterUnits", RegisterId::Units as u32),
            ("RaplRegisterPackageLimit", RegisterId::PackageLimit as u32),
            (
                "RaplRegisterMmioPackageLimit",
                RegisterId::MmioPackageLimit as u32,
            ),
        ] {
            assert_eq!(get(name), u64::from(flag), "{name}");
        }

        assert_eq!(get("RAPL_POWER_FIELD_MASK"), rapl::POWER_FIELD_MASK);
        assert_eq!(get("RAPL_LOCK_BIT"), rapl::LOCK_BIT);
        let (limit, pl1, pl2) = (
            get("RAPL_LIMIT_MASK"),
            get("RAPL_PL1_ENABLE"),
            get("RAPL_PL2_ENABLE"),
        );
        assert_eq!(limit | (limit << 32) | pl1 | pl2, rapl::POWER_FIELD_MASK);
        let units = Units::decode(get("RAPL_POWER_UNIT_MASK"));
        assert_eq!(units.watts_per_unit, 1.0 / 32768.0);
        let decoded = decode_limits(pl1 | limit, units);
        assert!(decoded.pl1.enabled && !decoded.pl2.enabled);
        assert_eq!(decoded.pl1.watts, limit as f64 * units.watts_per_unit);
        assert!(decode_limits(pl2, units).pl2.enabled);

        let link = DEVICE_PATH.trim_start_matches(r"\\.\");
        assert!(DRIVER_SOURCE.contains(&format!(r#"L"\\DosDevices\\{link}""#)));
    }
}
