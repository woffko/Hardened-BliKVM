//! The driver's interface, as `kernel/hardened-blikvm-ve/ve_enc_abi.h` has
//! it: one device, three requests. Nothing here is an address of the
//! engine's or a register value.

/// The driver's device node.
pub const DEVICE: &str = "/dev/hardened-blikvm-ve-enc";
pub const ABI: u32 = 1;
/// The visible picture: even numbers within these. The engine does not
/// finish a frame that refers to another when the picture is narrower than
/// 13 macroblocks; the driver takes none below 16.
pub const MIN_WIDTH: u32 = 256;
pub const MIN_HEIGHT: u32 = 64;
pub const MAX_WIDTH: u32 = 1920;
pub const MAX_HEIGHT: u32 = 1088;
/// A frame's header units: SPS, PPS, slice header.
pub const MAX_UNITS: usize = 3;
pub const MAX_UNIT_BYTES: usize = 128;
pub const MAX_QP: u32 = 51;

/// A configuration's flag.
pub const CABAC: u32 = 1 << 0;

/// A pass's flags. Every request is one pass of the engine over a picture;
/// the frame a new picture refers to is the last pass over the picture
/// before it unless the request says otherwise.
pub const KEY: u32 = 1 << 0;
/// The last picture once more, against the same reference.
pub const AGAIN: u32 = 1 << 1;
/// The engine's other search for motion.
pub const WIDE: u32 = 1 << 2;
/// A new picture refers to the pass before the last one.
pub const EARLIER: u32 = 1 << 3;
/// A new picture refers to what the picture before it referred to.
pub const STAY: u32 = 1 << 4;
pub const FRAME_FLAGS: u32 = KEY | AGAIN | WIDE | EARLIER | STAY;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Info {
    pub abi: u32,
    /// The engine's two identification registers.
    pub decoder_ip: u32,
    pub encoder_ip: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub reserved: u32,
    /// Out: the bytes of a picture, and the most a frame can have.
    pub picture_bytes: u32,
    pub stream_bytes: u32,
}

/// One unit of a frame's headers: the byte after the start code, and the
/// payload's bits, the most significant bit of the first byte first.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Unit {
    pub header: u8,
    pub reserved: u8,
    pub bits: u16,
    pub payload: [u8; MAX_UNIT_BYTES],
}

impl Default for Unit {
    fn default() -> Self {
        Self {
            header: 0,
            reserved: 0,
            bits: 0,
            payload: [0; MAX_UNIT_BYTES],
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Frame {
    pub flags: u32,
    pub qp: u32,
    /// NV12 of whole macroblocks; not read with `AGAIN`.
    pub picture: u64,
    /// `Unit[unit_count]`
    pub units: u64,
    /// Where the frame goes.
    pub stream: u64,
    pub picture_bytes: u32,
    pub unit_count: u32,
    pub stream_capacity: u32,
    /// Out. With `ENOSPC`: the bytes the frame has.
    pub stream_bytes: u32,
    /// The encoder's status bits as the interrupt found them.
    pub status: u32,
    pub encode_us: u32,
    /// The engine's numbers about the frame.
    pub statistics: [u32; 3],
    pub reserved: u32,
}

/// `_IOC` of the generic ioctl encoding: direction, size, type 'V', number.
const fn request(direction: u64, number: u64, size: usize) -> u64 {
    (direction << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | number
}
const READ: u64 = 2;
const WRITE_READ: u64 = 3;
pub const IOC_INFO: u64 = request(READ, 0x20, size_of::<Info>());
pub const IOC_CONFIGURE: u64 = request(WRITE_READ, 0x21, size_of::<Config>());
pub const IOC_ENCODE: u64 = request(WRITE_READ, 0x22, size_of::<Frame>());

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
        assert_eq!(FRAME_FLAGS, 0x1f);
    }
}
