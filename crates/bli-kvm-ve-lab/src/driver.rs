//! The encoder driver's device (`kernel/hardened-blikvm-ve/ve_enc_abi.h`)
//! with the lab tool as its client: the same pictures and the same headers
//! as through the lab module, but every register is the driver's. What the
//! two ways make of a sequence is compared byte for byte (gate V4).

use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

use crate::{
    headers::{self, Slice, Stream},
    picture::Picture,
    regs::Search,
    session::Encoded,
};

pub const DEVICE: &str = "/dev/hardened-blikvm-ve-enc";
const ABI: u32 = 1;
const MAX_UNITS: usize = 3;
const MAX_UNIT_BYTES: usize = 128;
const CABAC: u32 = 1 << 0;
const KEY: u32 = 1 << 0;
const AGAIN: u32 = 1 << 1;
const WIDE: u32 = 1 << 2;
const EARLIER: u32 = 1 << 3;
const STAY: u32 = 1 << 4;

/// Which pass over the picture before is the frame a new picture refers
/// to, as the driver is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// The last pass: nothing needs saying.
    Last,
    /// The pass before the last.
    Earlier,
    /// None of them: the picture was left out, and the stream stays where
    /// it was.
    Stay,
}

impl Reference {
    fn flags(self) -> u32 {
        match self {
            Self::Last => 0,
            Self::Earlier => EARLIER,
            Self::Stay => STAY,
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct Info {
    abi: u32,
    decoder_ip: u32,
    encoder_ip: u32,
    max_width: u32,
    max_height: u32,
    min_width: u32,
    min_height: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Default)]
struct Config {
    width: u32,
    height: u32,
    flags: u32,
    reserved: u32,
    picture_bytes: u32,
    stream_bytes: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Unit {
    header: u8,
    reserved: u8,
    bits: u16,
    payload: [u8; MAX_UNIT_BYTES],
}

#[repr(C)]
#[derive(Default)]
struct Frame {
    flags: u32,
    qp: u32,
    picture: u64,
    units: u64,
    stream: u64,
    picture_bytes: u32,
    unit_count: u32,
    stream_capacity: u32,
    stream_bytes: u32,
    status: u32,
    encode_us: u32,
    statistics: [u32; 3],
    reserved: u32,
}

/// `_IOC` of the generic ioctl encoding: direction, size, type 'V', number.
const fn request(direction: u64, number: u64, size: usize) -> u64 {
    (direction << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | number
}
const READ: u64 = 2;
const WRITE_READ: u64 = 3;
const IOC_INFO: u64 = request(READ, 0x20, size_of::<Info>());
const IOC_CONFIGURE: u64 = request(WRITE_READ, 0x21, size_of::<Config>());
const IOC_ENCODE: u64 = request(WRITE_READ, 0x22, size_of::<Frame>());

unsafe extern "C" {
    fn ioctl(descriptor: i32, request: u64, ...) -> i32;
}

/// One stream through the driver.
pub struct DriverStream {
    file: File,
    stream: Stream,
    picture_bytes: usize,
    /// The picture as the driver takes it: luma, then chroma.
    picture: Vec<u8>,
    output: Vec<u8>,
    frames: u32,
    frame_num: u32,
    idr_pic_id: u32,
    /// The search of the passes from here on; none is the driver's own.
    search: Option<Search>,
    /// Passes over the picture that is next, so far.
    passes: usize,
    /// What the next new picture refers to.
    reference: Reference,
    /// The engine's identifications as the driver read them.
    pub decoder_ip: u32,
    pub encoder_ip: u32,
    /// The engine's three numbers about the last frame.
    pub statistics: [u32; 3],
}

impl DriverStream {
    pub fn open(stream: Stream) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(DEVICE)?;
        let mut info = Info::default();
        // SAFETY: the request writes one `Info` to the address given.
        if unsafe { ioctl(file.as_raw_fd(), IOC_INFO, &mut info as *mut Info) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if info.abi != ABI {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the driver is not the one this tool was built for",
            ));
        }
        let mut config = Config {
            width: stream.width,
            height: stream.height,
            flags: if stream.cabac { CABAC } else { 0 },
            ..Config::default()
        };
        // SAFETY: the request reads and writes one `Config` at the address.
        if unsafe { ioctl(file.as_raw_fd(), IOC_CONFIGURE, &mut config as *mut Config) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let expected = (stream.coded_width() * stream.coded_height() * 3 / 2) as usize;
        if config.picture_bytes as usize != expected || config.stream_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the driver's picture is not the lab's",
            ));
        }
        Ok(Self {
            file,
            stream,
            picture_bytes: expected,
            picture: vec![0; expected],
            output: vec![0; config.stream_bytes as usize],
            frames: 0,
            frame_num: 0,
            idr_pic_id: 0,
            search: None,
            passes: 0,
            reference: Reference::Last,
            decoder_ip: info.decoder_ip,
            encoder_ip: info.encoder_ip,
            statistics: [0; 3],
        })
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// The passes from here on search like this; none is the driver's own
    /// way, the exact one.
    pub fn set_search(&mut self, search: Option<Search>) {
        self.search = search;
    }

    /// One pass over the frame that is next, as `Session::pass`: with
    /// `fresh` the picture is sent, without it the driver encodes the one it
    /// has again.
    pub fn pass(
        &mut self,
        picture: &Picture,
        key: bool,
        qp: i32,
        fresh: bool,
    ) -> Result<Encoded, String> {
        let began = Instant::now();
        if picture.bytes() != self.picture_bytes {
            return Err("the picture is not of the stream's size".into());
        }
        let key = key || self.frames == 0;
        let slice = Slice {
            key,
            frame_num: if key { 0 } else { self.frame_num },
            idr_pic_id: self.idr_pic_id,
            qp,
            cabac_init_idc: 0,
        };
        let mut header_ops = Vec::new();
        if key {
            header_ops.extend(headers::sps(&self.stream));
            header_ops.extend(headers::pps(&self.stream));
        }
        header_ops.extend(headers::slice_header(&self.stream, &slice));
        let raw = headers::units(&header_ops);
        if raw.len() > MAX_UNITS || raw.iter().any(|unit| unit.payload.len() > MAX_UNIT_BYTES) {
            return Err("the frame's headers are more than the driver takes".into());
        }
        let mut units = [Unit {
            header: 0,
            reserved: 0,
            bits: 0,
            payload: [0; MAX_UNIT_BYTES],
        }; MAX_UNITS];
        for (unit, raw) in units.iter_mut().zip(&raw) {
            unit.header = raw.header;
            unit.bits = raw.bits as u16;
            unit.payload[..raw.payload.len()].copy_from_slice(&raw.payload);
        }
        if fresh {
            self.picture[..picture.luma.len()].copy_from_slice(&picture.luma);
            self.picture[picture.luma.len()..].copy_from_slice(&picture.chroma);
        }
        // A first pass over a picture says which pass over the picture
        // before it is the frame; a later one is the picture again.
        let again = self.passes > 0;
        if again == fresh {
            return Err("a picture is new with its first pass and with no other".into());
        }
        let mut frame = Frame {
            flags: if key { KEY } else { 0 }
                | if again { AGAIN } else { self.reference.flags() }
                | if self.search == Some(Search::Wide) {
                    WIDE
                } else {
                    0
                },
            qp: qp as u32,
            picture: if fresh {
                self.picture.as_ptr() as u64
            } else {
                0
            },
            units: units.as_ptr() as u64,
            stream: self.output.as_mut_ptr() as u64,
            picture_bytes: self.picture_bytes as u32,
            unit_count: raw.len() as u32,
            stream_capacity: self.output.len() as u32,
            ..Frame::default()
        };
        // SAFETY: the request reads and writes one `Frame` at the address;
        // the three buffers it names live across the call and have the
        // lengths it says.
        if unsafe { ioctl(self.file.as_raw_fd(), IOC_ENCODE, &mut frame as *mut Frame) } != 0 {
            let error = io::Error::last_os_error();
            return Err(format!(
                "the driver refused the frame: {error}; status {:#x}",
                frame.status
            ));
        }
        self.statistics = frame.statistics;
        self.passes += 1;
        Ok(Encoded {
            key,
            qp,
            bytes: self.output[..frame.stream_bytes as usize].to_vec(),
            status: frame.status,
            // For the driver a pass is known by its place among the
            // picture's passes.
            set: self.passes - 1,
            time: Duration::from_micros(frame.encode_us as u64),
            total: began.elapsed(),
        })
    }

    /// As `Session::keep`. The driver never writes over the last pass, and
    /// a pass after it is the frame or the picture is left out: nothing
    /// needs saying.
    pub fn keep(&mut self, _encoded: &Encoded) {}

    /// A frame that says "nothing changed" in the place of the frame that
    /// is next, as `Session::unchanged`: the driver is not asked, and is
    /// told with the next picture that the stream stayed where it was.
    pub fn unchanged(&mut self) -> Encoded {
        let began = Instant::now();
        let bytes = crate::standin::unchanged_frame(&self.stream, self.frame_num);
        self.frame_num += 1;
        if self.passes > 0 {
            self.reference = Reference::Stay;
        }
        self.passes = 0;
        Encoded {
            key: false,
            qp: headers::INITIAL_QP,
            bytes,
            status: 0,
            set: 0,
            time: Duration::ZERO,
            total: began.elapsed(),
        }
    }

    /// The pass is the frame: the next picture refers to it, and its
    /// headers count on from it. It is the last pass over its picture or
    /// the one before it.
    pub fn accept(&mut self, encoded: &Encoded) {
        self.reference = if encoded.set + 1 == self.passes {
            Reference::Last
        } else {
            assert_eq!(
                encoded.set + 2,
                self.passes,
                "the driver has the last two passes"
            );
            Reference::Earlier
        };
        self.passes = 0;
        self.frames += 1;
        self.frame_num = if encoded.key { 1 } else { self.frame_num + 1 };
        if encoded.key {
            self.idr_pic_id ^= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requests_are_those_of_the_drivers_header() {
        // _IOR('V', 0x20, 32 bytes), _IOWR('V', 0x21, 24 bytes),
        // _IOWR('V', 0x22, 72 bytes); a unit is 132 bytes.
        assert_eq!(size_of::<Info>(), 32);
        assert_eq!(size_of::<Config>(), 24);
        assert_eq!(size_of::<Unit>(), 132);
        assert_eq!(size_of::<Frame>(), 72);
        assert_eq!(IOC_INFO, 0x8020_5620);
        assert_eq!(IOC_CONFIGURE, 0xc018_5621);
        assert_eq!(IOC_ENCODE, 0xc048_5622);
    }
}
