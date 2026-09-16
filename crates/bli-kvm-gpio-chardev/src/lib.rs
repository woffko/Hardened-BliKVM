//! Linux GPIO character-device driver.
//!
//! Implements the platform [`GpioDriver`] seam over the v1 line-handle ioctls
//! of `/dev/gpiochipN`. It requests one line at a time with the reviewed
//! consumer label, drives requested values and releases handles by closing
//! them. Only the ABI layout, flag mapping and error paths are host-tested; the
//! ioctl calls themselves are verified only on the designated board, so this
//! crate stays behind the reviewed physical line handoff.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;

use bli_kvm_platform_bli::{BOARD_CONSUMER, GpioDriver, LineDirection, LineRequest};
use thiserror::Error;

/// GPIO_GET_CHIPINFO_IOCTL from the Linux GPIO character-device UAPI.
pub const GET_CHIPINFO_IOCTL: u32 = 0x8044_1b01;

pub const GPIOHANDLE_REQUEST_INPUT: u32 = 0x01;
pub const GPIOHANDLE_REQUEST_OUTPUT: u32 = 0x02;
pub const GPIOHANDLE_REQUEST_ACTIVE_LOW: u32 = 0x04;

const MAX_LINES: usize = 64;
const CHIP_NAME_LEN: usize = 32;
const CHIP_LABEL_LEN: usize = 32;

/// `struct gpiochip_info` from the Linux GPIO character-device UAPI.
#[repr(C)]
#[derive(Clone, Copy)]
struct GpiochipInfo {
    name: [u8; CHIP_NAME_LEN],
    label: [u8; CHIP_LABEL_LEN],
    lines: u32,
}

impl Default for GpiochipInfo {
    fn default() -> Self {
        Self {
            name: [0; CHIP_NAME_LEN],
            label: [0; CHIP_LABEL_LEN],
            lines: 0,
        }
    }
}
const CONSUMER_LABEL_LEN: usize = 32;
const MAX_CONSUMER_LEN: usize = CONSUMER_LABEL_LEN - 1;

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
const GPIO_IOC_TYPE: u32 = 0xB4;
const GET_LINEHANDLE_NR: u32 = 0x03;
const SET_LINE_VALUES_NR: u32 = 0x09;

#[repr(C)]
#[derive(Clone, Copy)]
struct GpiohandleRequest {
    lineoffsets: [u32; MAX_LINES],
    flags: u32,
    default_values: [u8; MAX_LINES],
    consumer_label: [u8; CONSUMER_LABEL_LEN],
    lines: u32,
    fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GpiohandleData {
    values: [u8; MAX_LINES],
}

impl Default for GpiohandleData {
    fn default() -> Self {
        Self {
            values: [0; MAX_LINES],
        }
    }
}

const _: () = assert!(std::mem::size_of::<GpiohandleRequest>() == 364);
const _: () = assert!(std::mem::size_of::<GpiohandleData>() == 64);

const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> u64 {
    ((dir as u64) << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | nr as u64
}

const fn iowr(ty: u32, nr: u32, size: usize) -> u64 {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size)
}

const GET_LINEHANDLE_IOCTL: u64 = iowr(
    GPIO_IOC_TYPE,
    GET_LINEHANDLE_NR,
    std::mem::size_of::<GpiohandleRequest>(),
);
const SET_LINE_VALUES_IOCTL: u64 = iowr(
    GPIO_IOC_TYPE,
    SET_LINE_VALUES_NR,
    std::mem::size_of::<GpiohandleData>(),
);

#[derive(Debug, Error)]
pub enum GpioChardevError {
    #[error("GPIO character device I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("GPIO line was not requested")]
    NotRequested,
    #[error("GPIO consumer label is invalid")]
    InvalidConsumer,
}

impl From<GpioChardevError> for String {
    fn from(error: GpioChardevError) -> Self {
        error.to_string()
    }
}

/// Compute the request flags for a planned line.
fn request_flags(request: &LineRequest) -> u32 {
    let direction = match request.direction {
        LineDirection::Input => GPIOHANDLE_REQUEST_INPUT,
        LineDirection::Output => GPIOHANDLE_REQUEST_OUTPUT,
    };
    if request.active_low {
        direction | GPIOHANDLE_REQUEST_ACTIVE_LOW
    } else {
        direction
    }
}

fn consumer_label() -> Result<[u8; CONSUMER_LABEL_LEN], GpioChardevError> {
    let bytes = BOARD_CONSUMER.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_CONSUMER_LEN {
        return Err(GpioChardevError::InvalidConsumer);
    }
    let mut label = [0_u8; CONSUMER_LABEL_LEN];
    label[..bytes.len()].copy_from_slice(bytes);
    Ok(label)
}

/// Linux GPIO character-device driver over one chip.
pub struct ChardevGpioDriver {
    chip: File,
    consumer: [u8; CONSUMER_LABEL_LEN],
    /// Line count reported by the chip; the reviewed bound for every offset.
    chip_lines: u32,
    handles: BTreeMap<u16, RawFd>,
}

impl ChardevGpioDriver {
    pub fn open(chip_path: &Path) -> Result<Self, GpioChardevError> {
        let chip = OpenOptions::new().read(true).write(true).open(chip_path)?;
        let mut info = GpiochipInfo::default();
        let result = unsafe { libc::ioctl(chip.as_raw_fd(), GET_CHIPINFO_IOCTL as _, &mut info) };
        if result < 0 {
            return Err(GpioChardevError::Io(std::io::Error::last_os_error()));
        }
        if info.lines == 0 {
            return Err(GpioChardevError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "gpio chip reports no lines",
            )));
        }
        Ok(Self {
            chip,
            consumer: consumer_label()?,
            chip_lines: info.lines,
            handles: BTreeMap::new(),
        })
    }

    fn request(&mut self, request: &LineRequest) -> Result<RawFd, GpioChardevError> {
        if u32::from(request.offset) >= self.chip_lines {
            return Err(GpioChardevError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "line offset exceeds the chip's line count",
            )));
        }
        let mut payload = GpiohandleRequest {
            lineoffsets: [0; MAX_LINES],
            flags: request_flags(request),
            default_values: [0; MAX_LINES],
            consumer_label: self.consumer,
            lines: 1,
            fd: 0,
        };
        payload.lineoffsets[0] = u32::from(request.offset);
        let result = unsafe {
            libc::ioctl(
                self.chip.as_raw_fd(),
                GET_LINEHANDLE_IOCTL as _,
                &mut payload,
            )
        };
        if result < 0 {
            return Err(GpioChardevError::Io(std::io::Error::last_os_error()));
        }
        Ok(payload.fd)
    }

    fn handle(&self, offset: u16) -> Result<RawFd, GpioChardevError> {
        self.handles
            .get(&offset)
            .copied()
            .ok_or(GpioChardevError::NotRequested)
    }
}

#[cfg(test)]
impl ChardevGpioDriver {
    /// Test-only constructor: the chip-info ioctl cannot run against a plain
    /// file, so the tests supply the line count directly.
    pub(crate) fn with_chip_lines(
        chip_path: &Path,
        chip_lines: u32,
    ) -> Result<Self, GpioChardevError> {
        let chip = OpenOptions::new().read(true).write(true).open(chip_path)?;
        Ok(Self {
            chip,
            consumer: consumer_label()?,
            chip_lines,
            handles: BTreeMap::new(),
        })
    }
}

impl GpioDriver for ChardevGpioDriver {
    type Error = GpioChardevError;

    fn request_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
        if self.handles.contains_key(&request.offset) {
            return Ok(());
        }
        let fd = self.request(request)?;
        self.handles.insert(request.offset, fd);
        Ok(())
    }

    fn write_line(&mut self, request: &LineRequest, active: bool) -> Result<(), Self::Error> {
        let fd = self.handle(request.offset)?;
        let mut data = GpiohandleData::default();
        data.values[0] = u8::from(active);
        let result = unsafe { libc::ioctl(fd, SET_LINE_VALUES_IOCTL as _, &data) };
        if result < 0 {
            return Err(GpioChardevError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn release_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
        if let Some(fd) = self.handles.remove(&request.offset) {
            let result = unsafe { libc::close(fd) };
            if result < 0 {
                return Err(GpioChardevError::Io(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_platform_bli::{GpioRole, hardened_board_request};

    #[test]
    fn ioctl_numbers_match_the_kernel_abi() {
        // _IOWR(0xB4, 0x03, struct gpiohandle_request) and
        // _IOWR(0xB4, 0x09, struct gpiohandle_data).
        assert_eq!(GET_LINEHANDLE_IOCTL, 0xC16C_B403);
        assert_eq!(SET_LINE_VALUES_IOCTL, 0xC040_B409);
    }

    #[test]
    fn flags_follow_direction_and_polarity() {
        let power = hardened_board_request(GpioRole::AtxPower).unwrap();
        assert_eq!(request_flags(&power), GPIOHANDLE_REQUEST_OUTPUT);

        let mut active_low = power;
        active_low.active_low = true;
        assert_eq!(
            request_flags(&active_low),
            GPIOHANDLE_REQUEST_OUTPUT | GPIOHANDLE_REQUEST_ACTIVE_LOW
        );

        let mut input = power;
        input.direction = LineDirection::Input;
        assert_eq!(request_flags(&input), GPIOHANDLE_REQUEST_INPUT);
    }

    #[test]
    fn consumer_label_is_the_fixed_board_label() {
        let label = consumer_label().unwrap();
        let text = label
            .iter()
            .take_while(|byte| **byte != 0)
            .copied()
            .collect::<Vec<u8>>();
        assert_eq!(text, BOARD_CONSUMER.as_bytes());
        assert!(label.iter().skip(text.len()).all(|byte| *byte == 0));
    }

    #[test]
    fn opening_a_missing_chip_fails_cleanly() {
        assert!(matches!(
            ChardevGpioDriver::open(Path::new("/nonexistent/gpiochip")),
            Err(GpioChardevError::Io(_))
        ));
    }

    #[test]
    fn write_and_release_without_a_request_report_not_requested() {
        let chip_path = std::env::temp_dir().join("hardened-blikvm-gpio-chardev");
        let _ = std::fs::remove_file(&chip_path);
        std::fs::write(&chip_path, b"").unwrap();
        let mut driver = ChardevGpioDriver::with_chip_lines(&chip_path, 512).unwrap();
        let power = hardened_board_request(GpioRole::AtxPower).unwrap();
        assert!(matches!(
            driver.write_line(&power, true),
            Err(GpioChardevError::NotRequested)
        ));
        assert!(driver.release_line(&power).is_ok());
        let _ = std::fs::remove_file(&chip_path);
    }
}
