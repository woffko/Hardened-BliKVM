//! The board's agent collection on Windows: found among the HID collections
//! by the board's vendor and product and the collection's usage page, usage
//! and report lengths, and opened for overlapped reads and writes with the
//! user's rights (Windows' own HID driver, no other). Windows' calls are
//! this module's unsafe code.

use std::io;
use std::mem::{offset_of, size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::Duration;

use bli_kvm_target_agent::client::{Link, Report};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetAttributes,
    HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
    WriteFile,
};
use windows_sys::Win32::System::IO::{
    CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};
use windows_sys::core::GUID;

/// The board's USB device: the Linux Foundation's multifunction composite
/// gadget, as the board presents itself.
pub const VENDOR_ID: u16 = 0x1d6b;
pub const PRODUCT_ID: u16 = 0x0104;
/// The agent's collection.
const USAGE_PAGE: u16 = 0xff00;
const USAGE: u16 = 0x0001;
/// A report each way, its ID first.
const REPORT_LENGTH: u16 = bli_kvm_agent_protocol::REPORT_BYTES as u16;
/// How long one report may wait for the board.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

fn last_error() -> io::Error {
    // SAFETY: reads the calling thread's last error.
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn milliseconds(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX - 1)
}

/// The device path of the board's agent collection, zero-ended, if one is
/// present.
pub fn find() -> io::Result<Option<Vec<u16>>> {
    // SAFETY: each call gets what Windows documents for it: the class GUID
    // it filled, structures with their size set, and a detail buffer of the
    // size the first call asked for, aligned for the structure; the set is
    // destroyed once.
    unsafe {
        let mut guid: GUID = zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(
            &guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set == INVALID_HANDLE_VALUE as isize {
            return Err(last_error());
        }
        let mut found = None;
        for index in 0.. {
            let mut data: SP_DEVICE_INTERFACE_DATA = zeroed();
            data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, null(), &guid, index, &mut data) == 0 {
                break;
            }
            let mut required = 0_u32;
            SetupDiGetDeviceInterfaceDetailW(set, &data, null_mut(), 0, &mut required, null_mut());
            let path_offset = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
            if (required as usize) <= path_offset || required > 64 * 1024 {
                continue;
            }
            let mut buffer = vec![0_u64; (required as usize).div_ceil(8)];
            let detail = buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                &data,
                detail,
                required,
                null_mut(),
                null_mut(),
            ) == 0
            {
                continue;
            }
            let units = std::slice::from_raw_parts(
                buffer.as_ptr().cast::<u8>().add(path_offset).cast::<u16>(),
                (required as usize - path_offset) / 2,
            );
            let length = units
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(units.len());
            let mut path = units[..length].to_vec();
            path.push(0);
            if is_agent_collection(&path) {
                found = Some(path);
                break;
            }
        }
        SetupDiDestroyDeviceInfoList(set);
        Ok(found)
    }
}

/// Whether the collection at `path` is the board's agent collection: asked
/// without reading or writing it, which any collection allows.
fn is_agent_collection(path: &[u16]) -> bool {
    // SAFETY: `path` is zero-ended; the handle and the preparsed data are
    // freed on every way out.
    unsafe {
        let handle = CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut attributes: HIDD_ATTRIBUTES = zeroed();
        attributes.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        let mut matches = HidD_GetAttributes(handle, &mut attributes)
            && attributes.VendorID == VENDOR_ID
            && attributes.ProductID == PRODUCT_ID;
        if matches {
            let mut preparsed = 0;
            matches = HidD_GetPreparsedData(handle, &mut preparsed);
            if matches {
                let mut caps: HIDP_CAPS = zeroed();
                matches = HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS
                    && caps.UsagePage == USAGE_PAGE
                    && caps.Usage == USAGE
                    && caps.InputReportByteLength == REPORT_LENGTH
                    && caps.OutputReportByteLength == REPORT_LENGTH;
                HidD_FreePreparsedData(preparsed);
            }
        }
        CloseHandle(handle);
        matches
    }
}

/// A read that waits for the board: its buffer and overlapped structure
/// stay where they are until it ends.
struct PendingRead {
    overlapped: OVERLAPPED,
    buffer: Report,
}

/// The open agent collection.
pub struct HidLink {
    handle: HANDLE,
    write_event: HANDLE,
    read: Box<PendingRead>,
    reading: bool,
}

impl HidLink {
    /// Opens the collection at `path` (zero-ended) for reading and writing.
    pub fn open(path: &[u16]) -> io::Result<Self> {
        // SAFETY: `path` is zero-ended; what is created is closed by Drop or
        // here when a later step fails.
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(last_error());
            }
            let write_event = CreateEventW(null(), 1, 0, null());
            let read_event = CreateEventW(null(), 1, 0, null());
            if write_event.is_null() || read_event.is_null() {
                let error = last_error();
                for event in [write_event, read_event] {
                    if !event.is_null() {
                        CloseHandle(event);
                    }
                }
                CloseHandle(handle);
                return Err(error);
            }
            let mut read = Box::new(PendingRead {
                overlapped: zeroed(),
                buffer: [0; bli_kvm_agent_protocol::REPORT_BYTES],
            });
            read.overlapped.hEvent = read_event;
            Ok(Self {
                handle,
                write_event,
                read,
                reading: false,
            })
        }
    }
}

impl Link for HidLink {
    fn send(&mut self, report: &Report) -> io::Result<()> {
        // SAFETY: the overlapped structure and the report outlive the write:
        // it is waited for, or cancelled and then waited for, before this
        // returns.
        unsafe {
            let mut overlapped: OVERLAPPED = zeroed();
            overlapped.hEvent = self.write_event;
            ResetEvent(self.write_event);
            let mut written = 0_u32;
            if WriteFile(
                self.handle,
                report.as_ptr(),
                u32::from(REPORT_LENGTH),
                null_mut(),
                &mut overlapped,
            ) == 0
                && GetLastError() != ERROR_IO_PENDING
            {
                return Err(last_error());
            }
            if GetOverlappedResultEx(
                self.handle,
                &overlapped,
                &mut written,
                milliseconds(WRITE_TIMEOUT),
                0,
            ) == 0
            {
                let error = last_error();
                if error.raw_os_error() == Some(WAIT_TIMEOUT as i32) {
                    CancelIoEx(self.handle, &overlapped);
                    GetOverlappedResult(self.handle, &overlapped, &mut written, 1);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the board did not take the report",
                    ));
                }
                return Err(error);
            }
            if written != u32::from(REPORT_LENGTH) {
                return Err(io::Error::other("the report was not written whole"));
            }
            Ok(())
        }
    }

    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>> {
        // SAFETY: the read's buffer and overlapped structure are boxed and
        // stay until the read ends, here or in Drop.
        unsafe {
            if !self.reading {
                ResetEvent(self.read.overlapped.hEvent);
                let read = &mut *self.read;
                if ReadFile(
                    self.handle,
                    read.buffer.as_mut_ptr(),
                    u32::from(REPORT_LENGTH),
                    null_mut(),
                    &mut read.overlapped,
                ) == 0
                    && GetLastError() != ERROR_IO_PENDING
                {
                    return Err(last_error());
                }
                self.reading = true;
            }
            let mut read = 0_u32;
            if GetOverlappedResultEx(
                self.handle,
                &self.read.overlapped,
                &mut read,
                milliseconds(timeout),
                0,
            ) == 0
            {
                let error = last_error();
                if error.raw_os_error() == Some(WAIT_TIMEOUT as i32)
                    || error.raw_os_error() == Some(ERROR_IO_INCOMPLETE as i32)
                {
                    return Ok(None);
                }
                self.reading = false;
                return Err(error);
            }
            self.reading = false;
            if read != u32::from(REPORT_LENGTH) {
                return Ok(None);
            }
            Ok(Some(self.read.buffer))
        }
    }
}

impl Drop for HidLink {
    fn drop(&mut self) {
        // SAFETY: a read still waiting is cancelled and waited for before
        // its buffer goes; every handle is closed once.
        unsafe {
            if self.reading {
                let mut read = 0_u32;
                CancelIoEx(self.handle, &self.read.overlapped);
                GetOverlappedResult(self.handle, &self.read.overlapped, &mut read, 1);
            }
            CloseHandle(self.read.overlapped.hEvent);
            CloseHandle(self.write_event);
            CloseHandle(self.handle);
        }
    }
}
