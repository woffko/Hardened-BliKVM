//! The engine itself, through the lab module's device
//! (`kernel/hardened-blikvm-ve/ve_lab_abi.h`): the register window and the
//! buffers are mapped into this process, and the module tells when the
//! encoder's interrupt came.

use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
    ptr,
    time::Duration,
};

use crate::engine::Engine;

pub const DEVICE: &str = "/dev/hardened-blikvm-ve-lab";
const ABI: u32 = 1;
const BUFFER_SHIFT: u32 = 28;
const MAX_WAIT_MS: u128 = 10_000;

#[repr(C)]
#[derive(Default)]
struct Info {
    abi: u32,
    version: u32,
    window_bytes: u32,
    max_buffers: u32,
}

#[repr(C)]
#[derive(Default)]
struct Alloc {
    bytes: u32,
    index: u32,
    address: u32,
    reserved: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Default)]
struct Wait {
    timeout_ms: u32,
    came: u32,
    status: u32,
    unknown: u32,
}

/// `_IOC` of the generic ioctl encoding: direction, size, type 'V', number.
const fn request(direction: u64, number: u64, size: usize) -> u64 {
    (direction << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | number
}
const READ: u64 = 2;
const WRITE_READ: u64 = 3;
const IOC_INFO: u64 = request(READ, 0x10, size_of::<Info>());
const IOC_ALLOC: u64 = request(WRITE_READ, 0x11, size_of::<Alloc>());
const IOC_WAIT: u64 = request(WRITE_READ, 0x12, size_of::<Wait>());
const IOC_RESET: u64 = request(0, 0x13, 0);

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;

unsafe extern "C" {
    fn ioctl(descriptor: i32, request: u64, ...) -> i32;
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}

struct Mapping {
    pointer: *mut u8,
    bytes: usize,
    address: u32,
}

pub struct Device {
    file: File,
    registers: *mut u8,
    window: usize,
    version: u32,
    buffers: Vec<Mapping>,
    /// An interrupt came that was not the encoder's: the module switched
    /// the line off, and the session is over.
    pub unknown_interrupt: bool,
}

impl Device {
    /// Opens the device, which powers the engine, and maps its registers.
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(DEVICE)?;
        let mut info = Info::default();
        // SAFETY: the request writes one `Info` to the address given.
        if unsafe { ioctl(file.as_raw_fd(), IOC_INFO, &mut info as *mut Info) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if info.abi != ABI || info.window_bytes < 0x1000 || info.window_bytes > 0x10000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the lab module is not the one this tool was built for",
            ));
        }
        let window = info.window_bytes as usize;
        let registers = map(&file, window, 0)?;
        Ok(Self {
            file,
            registers,
            window,
            version: info.version,
            buffers: Vec::new(),
            unknown_interrupt: false,
        })
    }

    /// The version register as the module read it after power-on.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The bytes of the register window.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Resets the engine: every register back to its state after power-on.
    pub fn reset(&mut self) -> io::Result<()> {
        // SAFETY: the request takes no argument.
        if unsafe { ioctl(self.file.as_raw_fd(), IOC_RESET) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn register(&self, reg: u32) -> *mut u32 {
        let offset = reg as usize;
        assert!(
            offset % 4 == 0 && offset + 4 <= self.window,
            "register {reg:#x} is outside the window"
        );
        // SAFETY: the offset lies within the mapped window.
        unsafe { self.registers.add(offset).cast() }
    }
}

fn map(file: &File, bytes: usize, offset: u64) -> io::Result<*mut u8> {
    // SAFETY: a new shared mapping of the device; the kernel validates
    // length and offset.
    let pointer = unsafe {
        mmap(
            ptr::null_mut(),
            bytes,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            offset as i64,
        )
    };
    if pointer as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(pointer.cast())
}

impl Engine for Device {
    fn read(&mut self, reg: u32) -> u32 {
        // SAFETY: an aligned register inside the mapped window.
        unsafe { self.register(reg).read_volatile() }
    }

    fn write(&mut self, reg: u32, value: u32) {
        // SAFETY: an aligned register inside the mapped window.
        unsafe { self.register(reg).write_volatile(value) }
    }

    fn wait(&mut self, timeout: Duration) -> bool {
        let mut wait = Wait {
            timeout_ms: timeout.as_millis().min(MAX_WAIT_MS) as u32,
            ..Wait::default()
        };
        // SAFETY: the request reads and writes one `Wait` at the address.
        if unsafe { ioctl(self.file.as_raw_fd(), IOC_WAIT, &mut wait as *mut Wait) } != 0 {
            return false;
        }
        if wait.unknown != 0 {
            self.unknown_interrupt = true;
        }
        wait.came != 0
    }

    fn reset(&mut self) -> io::Result<()> {
        Device::reset(self)
    }

    fn allocate(&mut self, bytes: usize) -> io::Result<usize> {
        let mut alloc = Alloc {
            bytes: u32::try_from(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too large"))?,
            ..Alloc::default()
        };
        // SAFETY: the request reads and writes one `Alloc` at the address.
        if unsafe { ioctl(self.file.as_raw_fd(), IOC_ALLOC, &mut alloc as *mut Alloc) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if alloc.offset != (alloc.index as u64 + 1) << BUFFER_SHIFT || alloc.bytes < bytes as u32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the lab module gave an unexpected buffer",
            ));
        }
        let pointer = map(&self.file, alloc.bytes as usize, alloc.offset)?;
        self.buffers.push(Mapping {
            pointer,
            bytes: alloc.bytes as usize,
            address: alloc.address,
        });
        Ok(self.buffers.len() - 1)
    }

    fn address(&self, buffer: usize) -> u32 {
        self.buffers[buffer].address
    }

    fn bytes(&mut self, buffer: usize) -> &mut [u8] {
        let mapping = &self.buffers[buffer];
        // SAFETY: the mapping is `bytes` long, lives as long as `self`, and
        // `self` is borrowed for the slice's life.
        unsafe { std::slice::from_raw_parts_mut(mapping.pointer, mapping.bytes) }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        for mapping in &self.buffers {
            // SAFETY: a mapping made by `map` with this length.
            unsafe { munmap(mapping.pointer.cast(), mapping.bytes) };
        }
        // SAFETY: the register window's mapping.
        unsafe { munmap(self.registers.cast(), self.window) };
        // Closing the file resets the engine and frees the buffers.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requests_are_those_of_the_modules_header() {
        // _IOR('V', 0x10, 16 bytes), _IOWR('V', 0x11, 24 bytes),
        // _IOWR('V', 0x12, 16 bytes), _IO('V', 0x13).
        assert_eq!(size_of::<Info>(), 16);
        assert_eq!(size_of::<Alloc>(), 24);
        assert_eq!(size_of::<Wait>(), 16);
        assert_eq!(IOC_INFO, 0x8010_5610);
        assert_eq!(IOC_ALLOC, 0xc018_5611);
        assert_eq!(IOC_WAIT, 0xc010_5612);
        assert_eq!(IOC_RESET, 0x0000_5613);
    }
}
