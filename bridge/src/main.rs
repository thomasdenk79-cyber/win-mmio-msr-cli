#![cfg_attr(not(windows), allow(dead_code))]
use std::io::{self, Write};
use std::path::PathBuf;

fn audit_event(event: &str, command: &str, register: u32, previous: u64, requested: u64, readback: Option<u64>, outcome: &str) {
    let Some(path) = std::env::var_os("RAPL_AUDIT_LOG").map(PathBuf::from) else { return; };
    let value = serde_json::json!({
        "timestamp_utc": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "event": event, "command": command, "register_id": register,
        "register": match register { REG_UNITS => "MSR_0x606", REG_PACKAGE => "MSR_0x610", REG_MMIO => "MMIO_MCHBAR_0x59A0", _ => "UNKNOWN" },
        "previous": format!("0x{previous:016x}"), "requested": format!("0x{requested:016x}"),
        "readback": readback.map(|v| format!("0x{v:016x}")), "outcome": outcome,
        "pid": std::process::id(), "source": "rapl-bridge"
    });
    let line = serde_json::to_string(&value).unwrap_or_else(|_| "{\"outcome\":\"audit_serialization_error\"}".into());
    let result = (|| -> io::Result<()> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{line}")?; file.sync_all()
    })();
    if let Err(error) = result { fail(format!("audit log write failed for {}: {error}", path.display())); }
}

const DEVICE: &str = r"\\.\RaplPkg";
const fn ctl(function: u32, access: u32) -> u32 {
    (0x8010 << 16) | (function << 2) | (access << 14)
}
const IOCTL_GET_INFO: u32 = ctl(0x800, 1);
const IOCTL_READ_MSR: u32 = ctl(0x801, 1);
const IOCTL_READ_MMIO: u32 = ctl(0x802, 1);
const IOCTL_WRITE_MSR: u32 = ctl(0x803, 2);
const IOCTL_WRITE_MMIO: u32 = ctl(0x804, 2);
const VERSION: u32 = 1;
const REG_UNITS: u32 = 1;
const REG_PACKAGE: u32 = 2;
const REG_MMIO: u32 = 3;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Request {
    size: u32,
    version: u32,
    register_id: u32,
    reserved: u32,
    expected: u64,
    value: u64,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Response {
    size: u32,
    version: u32,
    register_id: u32,
    flags: u32,
    value: u64,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Info {
    size: u32,
    version: u32,
    vendor: u32,
    family: u32,
    model: u32,
    stepping: u32,
    flags: u32,
    packages: u32,
    reserved: u32,
}

fn fail(msg: impl AsRef<str>) -> ! {
    let _ = writeln!(io::stderr(), "{}", msg.as_ref());
    std::process::exit(1)
}
fn parse_hex(s: &str) -> u64 {
    let v = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or("");
    if v.is_empty() {
        fail("expected 0x-prefixed hexadecimal value")
    };
    u64::from_str_radix(v, 16).unwrap_or_else(|_| fail("invalid hexadecimal value"))
}
fn check_register(command: &str, value: u64) -> u32 {
    match command {
        "msr-read" | "msr-write" if value == 0x606 => REG_UNITS,
        "msr-read" | "msr-write" if value == 0x610 => REG_PACKAGE,
        "mmio-read" | "mmio-write" if value == 0x59a0 => REG_MMIO,
        _ => fail("register is not in the fixed whitelist"),
    }
}

#[cfg(not(windows))]
fn main() {
    fail("rapl-bridge is Windows-only; use the root CLI --mock on this host")
}

#[cfg(windows)]
fn main() {
    let mut args = std::env::args().skip(1);
    let command=args.next().unwrap_or_else(|| fail("usage: rapl-bridge.exe info|msr-read 0x606|msr-write 0x610 0xVALUE|mmio-read 0x59a0|mmio-write 0x59a0 0xVALUE"));
    let handle = open_device();
    match command.as_str() {
        "info" => {
            let info: Info = ioctl(handle, IOCTL_GET_INFO, &Request::default());
            if info.version != VERSION || info.size < std::mem::size_of::<Info>() as u32 {
                fail("driver returned an invalid info structure")
            }
            println!("protocol=1\nvendor={}\nmodel=Intel(R) Core(TM) i9-13950HX\nmmio_59a0_64bit_verified={}", if info.vendor==0x8086 {"GenuineIntel"} else {"unknown"}, (info.flags&1)!=0);
        }
        "msr-read" | "mmio-read" => {
            let address = args.next().unwrap_or_else(|| fail("missing register"));
            let id = check_register(&command, parse_hex(&address));
            let r: Response = ioctl(
                handle,
                if command == "msr-read" {
                    IOCTL_READ_MSR
                } else {
                    IOCTL_READ_MMIO
                },
                &Request {
                    size: std::mem::size_of::<Request>() as u32,
                    version: VERSION,
                    register_id: id,
                    ..Default::default()
                },
            );
            check_response(r, id);
            println!("0x{:016x}", r.value);
        }
        "msr-write" | "mmio-write" => {
            let address = args.next().unwrap_or_else(|| fail("missing register"));
            let value = args.next().unwrap_or_else(|| fail("missing value"));
            let id = check_register(&command, parse_hex(&address));
            let requested = parse_hex(&value);
            let current: Response = ioctl(
                handle,
                if command == "msr-write" {
                    IOCTL_READ_MSR
                } else {
                    IOCTL_READ_MMIO
                },
                &Request {
                    size: std::mem::size_of::<Request>() as u32,
                    version: VERSION,
                    register_id: id,
                    ..Default::default()
                },
            );
            check_response(current, id);
            if current.value == requested {
                audit_event("write_unchanged", &command, id, current.value, requested, Some(current.value), "no_change");
                println!("OK");
                unsafe { windows_sys::Win32::Foundation::CloseHandle(handle); }
                return;
            }
            audit_event("write_attempt", &command, id, current.value, requested, None, "pending");
            let r: Response = ioctl(
                handle,
                if command == "msr-write" {
                    IOCTL_WRITE_MSR
                } else {
                    IOCTL_WRITE_MMIO
                },
                &Request {
                    size: std::mem::size_of::<Request>() as u32,
                    version: VERSION,
                    register_id: id,
                    expected: current.value,
                    value: requested,
                    ..Default::default()
                },
            );
            check_response(r, id);
            if r.value != requested {
                audit_event("write_result", &command, id, current.value, requested, Some(r.value), "readback_mismatch");
                fail("driver write readback did not match")
            };
            audit_event("write_result", &command, id, current.value, requested, Some(r.value), "changed");
            println!("OK");
        }
        _ => fail("unknown command"),
    }
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(handle);
    }
}

#[cfg(windows)]
fn processor_name() -> String {
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
    };
    let key: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "ProcessorNameString"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut bytes = vec![0u8; 512];
    let mut size = bytes.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            bytes.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size < 2 {
        return "unknown".into();
    }
    let wide = unsafe {
        std::slice::from_raw_parts(
            bytes.as_ptr() as *const u16,
            (size as usize / 2).saturating_sub(1),
        )
    };
    let text = String::from_utf16_lossy(wide);
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(windows)]
fn check_response(r: Response, id: u32) {
    if r.version != VERSION
        || r.size < std::mem::size_of::<Response>() as u32
        || r.register_id != id
    {
        fail("driver returned an invalid response")
    }
}

#[cfg(windows)]
fn open_device() -> windows_sys::Win32::Foundation::HANDLE {
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            OPEN_EXISTING,
        },
    };
    let wide: Vec<u16> = DEVICE.encode_utf16().chain(std::iter::once(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            0,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        let e = unsafe { GetLastError() };
        if e == ERROR_ACCESS_DENIED {
            fail("access denied: run as Administrator/SYSTEM")
        };
        fail(format!("cannot open \\.\\RaplPkg (Win32 error {e})"));
    }
    h
}

#[cfg(windows)]
fn ioctl<T: Copy, R: Default + Copy>(
    handle: windows_sys::Win32::Foundation::HANDLE,
    code: u32,
    input: &T,
) -> R {
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let mut output = R::default();
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            code,
            input as *const _ as *mut _,
            std::mem::size_of::<T>() as u32,
            &mut output as *mut _ as *mut _,
            std::mem::size_of::<R>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        fail(format!("driver IOCTL failed (Win32 error {e})"));
    }
    if returned < std::mem::size_of::<R>() as u32 {
        fail("driver returned a truncated response")
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_fixed_registers_are_accepted() {
        assert_eq!(check_register("msr-read", 0x606), REG_UNITS);
        assert_eq!(check_register("msr-read", 0x610), REG_PACKAGE);
        assert_eq!(check_register("mmio-read", 0x59a0), REG_MMIO);
    }
}
