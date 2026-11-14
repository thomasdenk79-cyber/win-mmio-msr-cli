//! Windows device access: one handle per bridge call, opened with least privilege.
use crate::abi::{
    describe_win32_error, Info, RegisterId, Request, Response, DEVICE_PATH, IOCTL_GET_INFO,
};
use crate::Driver;
use std::ffi::c_void;
use std::mem::size_of;
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING},
    System::{
        Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
        IO::DeviceIoControl,
    },
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct Device {
    handle: HANDLE,
}

impl Device {
    /// Read commands get a read-only handle, so the I/O manager rejects write IOCTLs.
    pub fn open(write: bool) -> Result<Self, String> {
        let path = wide(DEVICE_PATH);
        let access = if write {
            GENERIC_READ | GENERIC_WRITE
        } else {
            GENERIC_READ
        };
        // SAFETY: `path` is NUL-terminated and outlives the call; all other
        // arguments are plain values or null pointers accepted by CreateFileW.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // SAFETY: no preconditions.
            let code = unsafe { GetLastError() };
            return Err(format!(
                "Cannot open {DEVICE_PATH}: {}",
                describe_win32_error(code)
            ));
        }
        Ok(Self { handle })
    }

    fn control<O: Default + Copy>(&self, code: u32, input: Option<&Request>) -> Result<O, String> {
        let mut output = O::default();
        let mut returned = 0u32;
        let (input_ptr, input_len) = match input {
            Some(request) => (
                request as *const Request as *const c_void,
                size_of::<Request>() as u32,
            ),
            None => (std::ptr::null(), 0),
        };
        // SAFETY: the input and output buffers are valid for the given sizes for
        // the duration of this synchronous (non-overlapped) call.
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                code,
                input_ptr,
                input_len,
                &mut output as *mut O as *mut c_void,
                size_of::<O>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // SAFETY: no preconditions.
            let error = unsafe { GetLastError() };
            return Err(format!(
                "Driver request failed: {}",
                describe_win32_error(error)
            ));
        }
        if returned as usize != size_of::<O>() {
            return Err(format!(
                "The driver returned {returned} bytes; expected {}",
                size_of::<O>()
            ));
        }
        Ok(output)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `Device::open` and is closed once.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

impl Driver for Device {
    fn info(&mut self) -> Result<Info, String> {
        self.control::<Info>(IOCTL_GET_INFO, None)?.validated()
    }

    fn read(&mut self, register: RegisterId) -> Result<u64, String> {
        let request = Request::new(register, 0, 0);
        let response: Response = self.control(register.read_ioctl(), Some(&request))?;
        response.value_for(register)
    }

    fn write(&mut self, register: RegisterId, expected: u64, value: u64) -> Result<u64, String> {
        let (_, code) = register
            .write_operation()
            .ok_or("The register is read-only")?;
        let request = Request::new(register, expected, value);
        let response: Response = self.control(code, Some(&request))?;
        response.value_for(register)
    }
}

/// Display name of CPU 0 from the registry; "unknown" when unavailable.
pub fn processor_name() -> String {
    let key = wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
    let value = wide("ProcessorNameString");
    let mut buffer = [0u16; 256];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: key and value are NUL-terminated; `size` holds the buffer size in
    // bytes and RRF_RT_REG_SZ guarantees a NUL-terminated result.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return "unknown".into();
    }
    let chars = (size as usize / 2).min(buffer.len());
    String::from_utf16_lossy(&buffer[..chars])
        .trim_end_matches('\0')
        .trim()
        .to_owned()
}
