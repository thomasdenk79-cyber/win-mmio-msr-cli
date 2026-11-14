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

/// Explains the Win32 errors that the driver's NTSTATUS codes map to.
pub fn describe_win32_error(code: u32) -> String {
    let hint = match code {
        2 | 3 => "the RaplPkg device does not exist (driver not installed or not started)",
        5 => "access denied (Administrator/SYSTEM required, or a driver gate refused the request)",
        50 => "not supported by the driver (CPU unsupported or feature gate disabled)",
        87 => "the driver rejected the request (invalid parameter or write policy violation)",
        1237 => "the register changed concurrently (expected value mismatch); re-run plan",
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
        assert_eq!(describe_win32_error(1), "Win32 error 1");
    }
}
