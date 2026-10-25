//! The driver's device (`kernel/hardened-blikvm-ve/ve_enc_abi.h`) as a
//! `Device`.

use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
    path::Path,
    time::Duration,
};

use crate::{
    abi,
    device::{Device, Made, Pass, Picture, Reference, Sizes},
    headers::RawUnit,
    rate::Search,
};

unsafe extern "C" {
    fn ioctl(descriptor: i32, request: u64, ...) -> i32;
}

/// The open device. One client at a time: a second open fails.
#[derive(Debug)]
pub struct Driver {
    file: File,
    sizes: Option<Sizes>,
    /// The engine's identifications as the driver read them.
    pub decoder_ip: u32,
    pub encoder_ip: u32,
}

impl Driver {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut info = abi::Info::default();
        // SAFETY: the request writes one `Info` to the address given.
        if unsafe { ioctl(file.as_raw_fd(), abi::IOC_INFO, &mut info as *mut abi::Info) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if info.abi != abi::ABI {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the driver is not the one this client was built for",
            ));
        }
        Ok(Self {
            file,
            sizes: None,
            decoder_ip: info.decoder_ip,
            encoder_ip: info.encoder_ip,
        })
    }
}

impl Device for Driver {
    fn configure(&mut self, width: u32, height: u32, cabac: bool) -> io::Result<Sizes> {
        self.sizes = None;
        let mut config = abi::Config {
            width,
            height,
            flags: if cabac { abi::CABAC } else { 0 },
            ..abi::Config::default()
        };
        // SAFETY: the request reads and writes one `Config` at the address.
        if unsafe {
            ioctl(
                self.file.as_raw_fd(),
                abi::IOC_CONFIGURE,
                &mut config as *mut abi::Config,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let sizes = Sizes {
            picture_bytes: config.picture_bytes as usize,
            stream_bytes: config.stream_bytes as usize,
        };
        self.sizes = Some(sizes);
        Ok(sizes)
    }

    fn encode(
        &mut self,
        pass: &Pass<'_>,
        units: &[RawUnit],
        stream: &mut Vec<u8>,
    ) -> io::Result<Made> {
        let refused = || io::Error::from(io::ErrorKind::InvalidInput);
        let sizes = self.sizes.ok_or_else(refused)?;
        if units.len() > abi::MAX_UNITS
            || units
                .iter()
                .any(|unit| unit.payload.len() > abi::MAX_UNIT_BYTES)
        {
            return Err(refused());
        }
        let mut raw = [abi::Unit::default(); abi::MAX_UNITS];
        for (raw, unit) in raw.iter_mut().zip(units) {
            raw.header = unit.header;
            raw.bits = unit.bits as u16;
            raw.payload[..unit.payload.len()].copy_from_slice(&unit.payload);
        }
        let (picture, flags) = match pass.picture {
            Picture::New { nv12, reference } => {
                if nv12.len() != sizes.picture_bytes {
                    return Err(refused());
                }
                let reference = match reference {
                    Reference::Last => 0,
                    Reference::Earlier => abi::EARLIER,
                    Reference::Stay => abi::STAY,
                };
                (nv12.as_ptr() as u64, reference)
            }
            Picture::Again => (0, abi::AGAIN),
        };
        stream.clear();
        stream.reserve(sizes.stream_bytes);
        let mut frame = abi::Frame {
            flags: flags
                | if pass.key { abi::KEY } else { 0 }
                | if pass.search == Search::Wide {
                    abi::WIDE
                } else {
                    0
                },
            qp: pass.qp,
            picture,
            units: raw.as_ptr() as u64,
            stream: stream.as_mut_ptr() as u64,
            picture_bytes: sizes.picture_bytes as u32,
            unit_count: units.len() as u32,
            stream_capacity: stream.capacity().min(u32::MAX as usize) as u32,
            ..abi::Frame::default()
        };
        // SAFETY: the request reads and writes one `Frame` at the address;
        // the picture, the units and the stream's room live across the
        // call and have the lengths it says.
        if unsafe {
            ioctl(
                self.file.as_raw_fd(),
                abi::IOC_ENCODE,
                &mut frame as *mut abi::Frame,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let bytes = frame.stream_bytes as usize;
        if bytes > stream.capacity() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the driver says more bytes than there was room for",
            ));
        }
        // SAFETY: the driver wrote `bytes` bytes at the buffer's beginning,
        // within the capacity it was told.
        unsafe { stream.set_len(bytes) };
        Ok(Made {
            status: frame.status,
            time: Duration::from_micros(frame.encode_us as u64),
            statistics: frame.statistics,
        })
    }
}
