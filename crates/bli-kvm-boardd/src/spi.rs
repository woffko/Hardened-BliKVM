//! SPI transport for the ST7789 panel.
//!
//! The ioctl numbers are computed from the Linux ABI at compile time and pinned
//! by a test, the same discipline the GPIO character-device driver uses.

use std::{
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
    path::Path,
};

use thiserror::Error;

/// `_IOW(type, nr, size)` with `size` bytes.
const fn iow(kind: u8, nr: u8, size: u32) -> u64 {
    const DIR_WRITE: u64 = 1;
    const IOC_NRSHIFT: u64 = 0;
    const IOC_TYPESHIFT: u64 = 8;
    const IOC_SIZESHIFT: u64 = 16;
    const IOC_DIRSHIFT: u64 = 30;
    (DIR_WRITE << IOC_DIRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
        | ((kind as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
}

/// SPI ioctls from `linux/spi/spidev.h`.
pub const SPI_IOC_WR_MODE: u64 = iow(b'k', 1, 1);
pub const SPI_IOC_WR_BITS_PER_WORD: u64 = iow(b'k', 3, 1);
pub const SPI_IOC_WR_MAX_SPEED_HZ: u64 = iow(b'k', 4, 4);
/// `SPI_IOC_MESSAGE(1)`, one `spi_ioc_transfer` (32 bytes on this ABI).
pub const SPI_IOC_MESSAGE_1: u64 = iow(b'k', 0, 32);

/// Linux `struct spi_ioc_transfer` for a 64-bit target.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct SpiIocTransfer {
    tx_buf: u64,
    rx_buf: u64,
    len: u32,
    speed_hz: u32,
    delay_usecs: u16,
    bits_per_word: u8,
    cs_change: u8,
    tx_nbits: u8,
    rx_nbits: u8,
    word_delay_usecs: u8,
    pad: u8,
}

#[derive(Debug, Error)]
pub enum SpiError {
    #[error("spi device is unavailable")]
    Unavailable,
    #[error("spi configuration failed")]
    Configuration,
    #[error("spi transfer failed")]
    Transfer,
    #[error("spi transfer is too large")]
    TooLarge,
}

/// Transport a display driver writes to.
pub trait SpiBus: Send {
    /// Transfer bytes; `data` selects the controller's data/command state when
    /// the bus owns that line.
    fn write(&mut self, data: bool, bytes: &[u8]) -> Result<(), SpiError>;
}

/// Largest single transfer the daemon will issue.
pub const MAX_SPI_TRANSFER_BYTES: usize = 4096;

/// `/dev/spidevX.Y` transport: mode 0, 8 bits per word, fixed clock.
pub struct SpidevBus {
    file: File,
    speed_hz: u32,
}

impl SpidevBus {
    pub fn open(path: &Path, speed_hz: u32) -> Result<Self, SpiError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| SpiError::Unavailable)?;
        let bus = Self { file, speed_hz };
        bus.configure()?;
        Ok(bus)
    }

    fn configure(&self) -> Result<(), SpiError> {
        let fd = self.file.as_raw_fd();
        let mode: u8 = 0;
        let bits: u8 = 8;
        let speed = self.speed_hz;
        // Safety: the ioctls take a pointer to a value of the documented type.
        unsafe {
            if libc::ioctl(fd, SPI_IOC_WR_MODE as _, &mode) < 0
                || libc::ioctl(fd, SPI_IOC_WR_BITS_PER_WORD as _, &bits) < 0
                || libc::ioctl(fd, SPI_IOC_WR_MAX_SPEED_HZ as _, &speed) < 0
            {
                return Err(SpiError::Configuration);
            }
        }
        Ok(())
    }
}

impl SpiBus for SpidevBus {
    fn write(&mut self, _data: bool, bytes: &[u8]) -> Result<(), SpiError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > MAX_SPI_TRANSFER_BYTES {
            return Err(SpiError::TooLarge);
        }
        let mut transfer = SpiIocTransfer {
            tx_buf: bytes.as_ptr() as u64,
            len: bytes.len() as u32,
            speed_hz: self.speed_hz,
            bits_per_word: 8,
            ..SpiIocTransfer::default()
        };
        // Safety: the transfer describes exactly the slice above and lives for
        // the duration of the ioctl.
        let result = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                SPI_IOC_MESSAGE_1 as _,
                &mut transfer as *mut SpiIocTransfer,
            )
        };
        if result < 0 {
            return Err(SpiError::Transfer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_the_linux_abi() {
        // Values from linux/spi/spidev.h on this ABI, computed independently.
        assert_eq!(SPI_IOC_WR_MODE, 0x4001_6b01);
        assert_eq!(SPI_IOC_WR_BITS_PER_WORD, 0x4001_6b03);
        assert_eq!(SPI_IOC_WR_MAX_SPEED_HZ, 0x4004_6b04);
        assert_eq!(SPI_IOC_MESSAGE_1, 0x4020_6b00);
    }

    #[test]
    fn missing_device_fails_closed() {
        assert!(matches!(
            SpidevBus::open(Path::new("/nonexistent/spidev"), 1_000_000),
            Err(SpiError::Unavailable)
        ));
    }
}
